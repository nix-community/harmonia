// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! macOS runtime root scanning via libproc syscalls, instead of shelling
//! out to lsof.

// libproc has no safe wrapper in our dependency set. The FFI itself is
// unavoidable here. Every call site keeps its buffer handling local.
#![allow(unsafe_code)]

use super::{BlobScanner, add_unchecked};
use crate::HashSet;
use std::ffi::CStr;
use std::os::raw::{c_int, c_void};

const PROC_ALL_PIDS: u32 = 1;
const PROC_PIDLISTFDS: c_int = 1;
const PROC_PIDVNODEPATHINFO: c_int = 9;
const PROC_PIDREGIONPATHINFO: c_int = 8;
// proc_info.h: PROC_PIDFDVNODEINFO is 1 and carries no path;
// PATHINFO is 2.
const PROC_PIDFDVNODEPATHINFO: c_int = 2;
const PROX_FDTYPE_VNODE: u32 = 1;
const PROC_PIDPATHINFO_MAXSIZE: usize = 4 * 1024;
const MAXPATHLEN: usize = 1024;
const CTL_KERN: c_int = 1;
const KERN_PROCARGS2: c_int = 49;
const KERN_ARGMAX: c_int = 8;

unsafe extern "C" {
    fn proc_listpids(type_: u32, typeinfo: u32, buffer: *mut c_void, buffersize: c_int) -> c_int;
    fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    fn proc_pidinfo(
        pid: c_int,
        flavor: c_int,
        arg: u64,
        buffer: *mut c_void,
        buffersize: c_int,
    ) -> c_int;
    fn proc_pidfdinfo(
        pid: c_int,
        fd: c_int,
        flavor: c_int,
        buffer: *mut c_void,
        buffersize: c_int,
    ) -> c_int;
    fn sysctl(
        name: *mut c_int,
        namelen: u32,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *mut c_void,
        newlen: usize,
    ) -> c_int;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ProcFdInfo {
    proc_fd: i32,
    proc_fdtype: u32,
}

/// Layout of the prefix of struct vnode_info_path / vnode_fdinfowithpath /
/// proc_regionwithpathinfo: lots of opaque fields, then a NUL-terminated
/// path at a known offset. We only read the path. A fixed-size scratch
/// buffer avoids replicating the full struct definitions.
fn extract_cstr_path(buf: &[u8]) -> Option<String> {
    // The path is the trailing MAXPATHLEN bytes, up to the first NUL.
    if buf.len() < MAXPATHLEN {
        return None;
    }
    let path_bytes = &buf[buf.len() - MAXPATHLEN..];
    let cstr = CStr::from_bytes_until_nul(path_bytes).ok()?;
    let s = cstr.to_str().ok()?;
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn list_pids() -> Vec<i32> {
    unsafe {
        let count = proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0);
        if count <= 0 {
            return Vec::new();
        }
        let mut pids = vec![0i32; count as usize / std::mem::size_of::<i32>()];
        let bytes = proc_listpids(
            PROC_ALL_PIDS,
            0,
            pids.as_mut_ptr() as *mut c_void,
            (pids.len() * std::mem::size_of::<i32>()) as c_int,
        );
        if bytes <= 0 {
            return Vec::new();
        }
        pids.truncate(bytes as usize / std::mem::size_of::<i32>());
        pids.retain(|&p| p > 0);
        pids
    }
}

fn pid_exe(pid: i32, store_prefix: &str, unchecked: &mut HashSet<String>) {
    let mut buf = vec![0u8; PROC_PIDPATHINFO_MAXSIZE];
    let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if n > 0
        && let Ok(s) = std::str::from_utf8(&buf[..n as usize])
    {
        add_unchecked(store_prefix, s, unchecked);
    }
}

/// proc_vnodepathinfo holds cwd then root, each ending with a path.
/// sizeof(struct vnode_info_path) = sizeof(struct vnode_info)=152 + MAXPATHLEN = 1176.
const VNODE_INFO_PATH_SIZE: usize = 152 + MAXPATHLEN;

fn pid_cwd_root(pid: i32, store_prefix: &str, unchecked: &mut HashSet<String>) {
    // struct proc_vnodepathinfo { vnode_info_path pvi_cdir; vnode_info_path pvi_rdir; }
    let mut buf = vec![0u8; VNODE_INFO_PATH_SIZE * 2];
    let n = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDVNODEPATHINFO,
            0,
            buf.as_mut_ptr() as *mut c_void,
            buf.len() as c_int,
        )
    };
    // A short return would leave the path fields as our zero fill.
    if n as usize != VNODE_INFO_PATH_SIZE * 2 {
        return;
    }
    for chunk in buf.chunks(VNODE_INFO_PATH_SIZE) {
        if let Some(p) = extract_cstr_path(chunk) {
            add_unchecked(store_prefix, &p, unchecked);
        }
    }
}

/// sizeof(struct vnode_fdinfowithpath) = sizeof(proc_fileinfo)=24
/// + sizeof(vnode_info_path)=1176 = 1200.
const VNODE_FDINFO_SIZE: usize = 24 + VNODE_INFO_PATH_SIZE;

fn pid_fds(pid: i32, store_prefix: &str, unchecked: &mut HashSet<String>) {
    let n = unsafe { proc_pidinfo(pid, PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    if n <= 0 {
        return;
    }
    let count = n as usize / std::mem::size_of::<ProcFdInfo>();
    let mut fds = vec![
        ProcFdInfo {
            proc_fd: 0,
            proc_fdtype: 0
        };
        count
    ];
    let n = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDLISTFDS,
            0,
            fds.as_mut_ptr() as *mut c_void,
            (fds.len() * std::mem::size_of::<ProcFdInfo>()) as c_int,
        )
    };
    if n <= 0 {
        return;
    }
    let count = n as usize / std::mem::size_of::<ProcFdInfo>();
    let mut buf = vec![0u8; VNODE_FDINFO_SIZE];
    for fd in &fds[..count] {
        if fd.proc_fdtype != PROX_FDTYPE_VNODE {
            continue;
        }
        buf.fill(0);
        let r = unsafe {
            proc_pidfdinfo(
                pid,
                fd.proc_fd,
                PROC_PIDFDVNODEPATHINFO,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as c_int,
            )
        };
        if r as usize == VNODE_FDINFO_SIZE
            && let Some(p) = extract_cstr_path(&buf)
        {
            add_unchecked(store_prefix, &p, unchecked);
        }
    }
}

/// sizeof(struct proc_regionwithpathinfo) = sizeof(proc_regioninfo)=96
/// + sizeof(vnode_info_path)=1176 = 1272.
const PROC_REGIONINFO_SIZE: usize = 96;
const REGION_PATH_INFO_SIZE: usize = PROC_REGIONINFO_SIZE + VNODE_INFO_PATH_SIZE;
/// proc_regioninfo trailing fields: pri_address (u64), pri_size (u64).
const PRI_ADDRESS_OFFSET: usize = PROC_REGIONINFO_SIZE - 16;
const PRI_SIZE_OFFSET: usize = PROC_REGIONINFO_SIZE - 8;

fn pid_regions(pid: i32, store_prefix: &str, unchecked: &mut HashSet<String>) {
    let mut addr: u64 = 0;
    let mut buf = vec![0u8; REGION_PATH_INFO_SIZE];
    // Iterate region by region. Each call returns info for the region
    // containing/after `addr`, then addr is bumped past it. The strictly
    // increasing addr guarantees termination.
    loop {
        buf.fill(0);
        let r = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDREGIONPATHINFO,
                addr,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as c_int,
            )
        };
        if r as usize != REGION_PATH_INFO_SIZE {
            break;
        }
        let pri_address = u64::from_ne_bytes(
            buf[PRI_ADDRESS_OFFSET..PRI_ADDRESS_OFFSET + 8]
                .try_into()
                .unwrap(),
        );
        let pri_size = u64::from_ne_bytes(
            buf[PRI_SIZE_OFFSET..PRI_SIZE_OFFSET + 8]
                .try_into()
                .unwrap(),
        );
        if let Some(p) = extract_cstr_path(&buf) {
            add_unchecked(store_prefix, &p, unchecked);
        }
        let next = pri_address.saturating_add(pri_size.max(4096));
        if next <= addr {
            break;
        }
        addr = next;
    }
}

fn pid_environ(pid: i32, argmax: usize, scanner: &BlobScanner, unchecked: &mut HashSet<String>) {
    let mut mib = [CTL_KERN, KERN_PROCARGS2, pid];
    let mut buf = vec![0u8; argmax];
    let mut size = argmax;
    let r = unsafe {
        sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return;
    }
    // KERN_PROCARGS2 returns argc(4 bytes) + exec_path + NULs + argv + envp.
    // Just scan whole blob for store path substrings.
    let size = size.min(buf.len());
    scanner.scan(&buf[..size], unchecked);
}

fn kern_argmax() -> usize {
    let mut mib = [CTL_KERN, KERN_ARGMAX];
    let mut argmax: c_int = 0;
    let mut size = std::mem::size_of::<c_int>();
    let r = unsafe {
        sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            &mut argmax as *mut c_int as *mut c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r == 0 && argmax > 0 {
        argmax as usize
    } else {
        256 * 1024
    }
}

pub fn scan(store_prefix: &str, unchecked: &mut HashSet<String>) {
    let scanner = BlobScanner::new(store_prefix);
    let argmax = kern_argmax();
    for pid in list_pids() {
        pid_exe(pid, store_prefix, unchecked);
        pid_cwd_root(pid, store_prefix, unchecked);
        pid_fds(pid, store_prefix, unchecked);
        pid_regions(pid, store_prefix, unchecked);
        pid_environ(pid, argmax, &scanner, unchecked);
    }
}
