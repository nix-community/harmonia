use harmonia_daemon::config::Config;
use harmonia_daemon::error::{DaemonError, IoContext};
use harmonia_daemon::handler::LocalStoreHandler;
use harmonia_daemon::server::DaemonServer;
use std::path::PathBuf;
use tokio::signal;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<(), DaemonError> {
    // Initialize tracing subscriber
    tracing_subscriber::fmt::init();

    // Load configuration
    let config = match std::env::var("HARMONIA_DAEMON_CONFIG") {
        Ok(path) => Config::from_file(&PathBuf::from(path))?,
        Err(_) => Config::default(),
    };

    info!("Starting harmonia-daemon");
    info!("Socket path: {}", config.socket_path.display());
    info!("Store directory: {}", config.store_dir.display());
    info!("Database path: {}", config.db_path.display());

    // Create StoreDir from config
    let store_dir = harmonia_store_core::store_path::StoreDir::new(&config.store_dir)
        .map_err(|e| DaemonError::config(format!("Invalid store directory: {e}")))?;

    // If the store is on a read-only bind mount (typical NixOS default),
    // remount it writable so builders can create output paths.  This
    // mirrors nix-daemon's LocalStore::makeStoreWritable() and requires
    // the daemon to run as root.
    #[cfg(target_os = "linux")]
    if config.sandbox {
        make_store_writable(&config.store_dir)?;
    }

    // Create the local store handler
    let writable = config.sandbox;
    let mut handler = LocalStoreHandler::new(store_dir.clone(), config.db_path, writable).await?;

    // Configure sandbox for build isolation
    if config.sandbox {
        #[cfg(target_os = "linux")]
        let sandbox_config = {
            info!("Sandbox enabled (Linux, user namespaces)");
            harmonia_daemon::config::SandboxConfig::new_linux(config.pool_dir.clone())
        };
        #[cfg(not(target_os = "linux"))]
        let sandbox_config = {
            let group_name = config.build_users_group.as_deref().ok_or_else(|| {
                DaemonError::config(
                    "sandbox on macOS requires build_users_group to be set".to_string(),
                )
            })?;
            info!("Sandbox enabled (macOS, build users group: {group_name})");
            harmonia_daemon::config::SandboxConfig::from_group_name(
                config.pool_dir.clone(),
                group_name,
            )
            .map_err(|e| DaemonError::config(format!("sandbox config: {e}")))?
        };
        handler.set_sandbox_config(sandbox_config);
    }

    // Create and start the daemon server
    let server = DaemonServer::new(handler, config.socket_path.clone(), store_dir);

    // Set up signal handlers
    let shutdown = shutdown_signal();

    // Run the server
    tokio::select! {
        result = server.serve() => {
            if let Err(e) = result {
                error!("Server error: {e}");
                return Err(DaemonError::io("Server error", e));
            }
        }
        _ = shutdown => {
            info!("Received shutdown signal");
        }
    }

    // Clean up: remove socket file
    if config.socket_path.exists() {
        std::fs::remove_file(&config.socket_path).io_context(|| {
            format!(
                "Failed to remove socket file at {}",
                config.socket_path.display()
            )
        })?;
    }

    info!("harmonia-daemon stopped");
    Ok(())
}

/// Remount the Nix store read-write if it is currently mounted read-only.
///
/// On NixOS the store is bind-mounted with `ro,nosuid,nodev` by default.
/// The nix-daemon does the same remount in `LocalStore::makeStoreWritable()`.
/// Requires root privileges.
#[cfg(target_os = "linux")]
fn make_store_writable(store_dir: &std::path::Path) -> Result<(), DaemonError> {
    use nix::sys::statvfs::FsFlags;
    use nix::sys::statvfs::statvfs;

    if !nix::unistd::getuid().is_root() {
        return Ok(());
    }

    let stat = statvfs(store_dir)
        .map_err(|e| DaemonError::config(format!("statvfs({}): {e}", store_dir.display())))?;

    if stat.flags().contains(FsFlags::ST_RDONLY) {
        use nix::mount::{MsFlags, mount};

        // Make the store mount private so the writable remount does not
        // propagate to other mount namespaces (e.g. sandbox children).
        // Matches the defensive approach Nix takes inside sandboxes
        // (MS_PRIVATE | MS_REC on /).
        info!("Making {} mount private", store_dir.display());
        mount(
            None::<&str>,
            store_dir,
            None::<&str>,
            MsFlags::MS_PRIVATE,
            None::<&str>,
        )
        .map_err(|e| DaemonError::config(format!("make {} private: {e}", store_dir.display(),)))?;

        info!("Remounting {} read-write", store_dir.display());
        mount(
            None::<&str>,
            store_dir,
            None::<&str>,
            MsFlags::MS_REMOUNT | MsFlags::MS_BIND,
            None::<&str>,
        )
        .map_err(|e| {
            DaemonError::config(format!("remount {} writable: {e}", store_dir.display(),))
        })?;
    }

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
