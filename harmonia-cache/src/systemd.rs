//! Minimal systemd integration: socket activation (`sd_listen_fds`) and
//! readiness notification (`sd_notify`).

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use nix::fcntl::{FcntlArg, FdFlag, fcntl};
use nix::sys::socket::{AddressFamily, SockaddrLike, SockaddrStorage, getsockname};

use crate::error::{Result, ServerError};

const SD_LISTEN_FDS_START: RawFd = 3;

pub enum Listener {
    Tcp(std::net::TcpListener),
    Unix(std::os::unix::net::UnixListener),
}

/// `$VAR` is unset, or set and equal to our PID. An unset value is tolerated
/// so non-systemd launchers (`systemfd`, test harnesses) that cannot know the
/// child PID up front still work.
fn pid_var_matches(var: &str) -> bool {
    match std::env::var(var) {
        Err(_) => true,
        Ok(v) => v.parse::<u32>().ok() == Some(std::process::id()),
    }
}

/// Returns the inherited listener fds, empty when not activated.
pub fn inherited_fds() -> std::ops::Range<RawFd> {
    if !pid_var_matches("LISTEN_PID") {
        return SD_LISTEN_FDS_START..SD_LISTEN_FDS_START;
    }
    let count: RawFd = std::env::var("LISTEN_FDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    // LISTEN_* are left set: `remove_var` is unsound once tokio worker threads
    // exist; the LISTEN_PID check plus CLOEXEC on the fds protect grandchildren.
    SD_LISTEN_FDS_START..SD_LISTEN_FDS_START + count
}

/// Take ownership of an inherited socket fd. Must be called at most once per
/// fd value: it constructs an `OwnedFd`, so a second call would double-close.
pub fn classify(fd: RawFd) -> Result<Listener> {
    // SAFETY: systemd guarantees fds [3, 3+LISTEN_FDS) are open and owned by
    // us; we take ownership exactly once per fd.
    #[allow(unsafe_code)]
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };

    fcntl(&fd, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).map_err(|e| ServerError::Startup {
        reason: format!("F_SETFD on inherited fd: {e}"),
    })?;

    let addr: SockaddrStorage = getsockname(fd.as_raw_fd()).map_err(|e| ServerError::Startup {
        reason: format!("inherited fd is not a socket: {e}"),
    })?;

    let listener = match addr.family() {
        Some(AddressFamily::Inet) | Some(AddressFamily::Inet6) => {
            let l = std::net::TcpListener::from(fd);
            l.set_nonblocking(true).map_err(|e| ServerError::Startup {
                reason: format!("set_nonblocking on inherited fd: {e}"),
            })?;
            Listener::Tcp(l)
        }
        Some(AddressFamily::Unix) => {
            let l = std::os::unix::net::UnixListener::from(fd);
            l.set_nonblocking(true).map_err(|e| ServerError::Startup {
                reason: format!("set_nonblocking on inherited fd: {e}"),
            })?;
            Listener::Unix(l)
        }
        other => {
            return Err(ServerError::Startup {
                reason: format!("inherited fd has unsupported address family {other:?}"),
            }
            .into());
        }
    };
    Ok(listener)
}

fn notify_send(path: &str, state: &[u8]) -> std::io::Result<()> {
    let sock = std::os::unix::net::UnixDatagram::unbound()?;
    #[cfg(target_os = "linux")]
    if let Some(abs) = path.strip_prefix('@') {
        use nix::sys::socket::{MsgFlags, UnixAddr, sendto};
        let addr = UnixAddr::new_abstract(abs.as_bytes())?;
        sendto(sock.as_raw_fd(), state, &addr, MsgFlags::empty())?;
        return Ok(());
    }
    sock.send_to(state, path)?;
    Ok(())
}

/// Best-effort `sd_notify`; never fatal so standalone use is unaffected.
fn notify(state: &[u8]) {
    let Ok(path) = std::env::var("NOTIFY_SOCKET") else {
        return;
    };
    if let Err(e) = notify_send(&path, state) {
        tracing::warn!("sd_notify to {path} failed: {e}");
    }
}

pub fn notify_ready() {
    notify(b"READY=1");
}

/// Ping at half the systemd watchdog deadline. Only proves the async runtime
/// still schedules work; will not detect a single wedged worker.
pub fn spawn_watchdog() {
    if !pid_var_matches("WATCHDOG_PID") {
        return;
    }
    let Some(usec) = std::env::var("WATCHDOG_USEC")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&v| v > 0)
    else {
        return;
    };
    let interval =
        std::time::Duration::from_micros(usec / 2).max(std::time::Duration::from_secs(1));
    tracing::info!("systemd watchdog enabled, pinging every {interval:?}");
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            notify(b"WATCHDOG=1");
        }
    });
}
