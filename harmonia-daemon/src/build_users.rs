// SPDX-FileCopyrightText: 2026 Jörg Thalheim
// SPDX-License-Identifier: MIT

//! Build user allocation matching Nix's `user-lock.cc`.
//!
//! Both strategies use file locks so multiple daemon processes
//! coordinate without races. The lock is released when `UserLock`
//! is dropped.

mod auto_user_lock;
mod simple_user_lock;

pub use auto_user_lock::{acquire_auto_user_lock, auto_pool_dir};
pub use simple_user_lock::{acquire_simple_user_lock, simple_pool_dir};

use std::fs;

use nix::fcntl::Flock;

/// A held build-user lock. The file lock is released on drop.
pub struct UserLock {
    /// Kept open to hold the flock.
    _fd: Flock<fs::File>,
    first_uid: u32,
    first_gid: u32,
    nr_ids: u32,
}

impl UserLock {
    pub fn uid(&self) -> u32 {
        self.first_uid
    }

    pub fn gid(&self) -> u32 {
        self.first_gid
    }

    pub fn uid_count(&self) -> u32 {
        self.nr_ids
    }

    /// Get the UID range as (first, last) inclusive.
    pub fn uid_range(&self) -> (u32, u32) {
        (self.first_uid, self.first_uid + self.nr_ids - 1)
    }
}
