//! Memory-mapped file for zero-copy reads of immutable nix store files.
//!
//! # Safety
//!
//! The nix store guarantees that files are never modified or truncated after
//! creation, so read-only mappings cannot trigger SIGBUS from truncation.
#![allow(unsafe_code)]

use std::io;
use std::os::fd::AsFd;

/// A read-only memory-mapped file region.
/// POSIX guarantees that mmap adds its own reference to the underlying file
/// object, so the fd can be closed immediately after mapping (IEEE Std 1003.1).
pub struct MappedFile {
    ptr: *mut std::ffi::c_void,
    len: usize,
}

// SAFETY: The mapped memory is read-only and backed by immutable nix store files.
// The mapping lives as long as the MappedFile struct (unmapped on Drop).
// No mutable aliasing is possible since we only hand out shared &[u8] slices.
//
// Send: The pointer refers to kernel-managed page-cache memory that is safe to
// access from any thread — there is no thread-local state involved.
//
// Sync: All access is read-only (&[u8] slices), so concurrent reads from
// multiple threads are safe without synchronization.
unsafe impl Send for MappedFile {}
unsafe impl Sync for MappedFile {}

/// Files up to this size are read into a heap buffer; larger files are
/// memory-mapped for zero-copy reads.
pub const MMAP_THRESHOLD: u64 = 256 * 1024;

impl MappedFile {
    /// Memory-map a file for reading from an open file descriptor.
    pub fn from_fd(fd: &impl AsFd, size: u64) -> io::Result<Self> {
        if size == 0 {
            return Ok(Self {
                ptr: std::ptr::null_mut(),
                len: 0,
            });
        }

        let len = usize::try_from(size)
            .map_err(|_| io::Error::other("file too large to mmap on this platform"))?;

        // SAFETY: The nix store is immutable — files cannot be modified or
        // truncated while mapped, so SIGBUS from truncation is not possible.
        let ptr = unsafe {
            nix::sys::mman::mmap(
                None,
                std::num::NonZeroUsize::new(len)
                    .ok_or_else(|| io::Error::other("file size is 0"))?,
                nix::sys::mman::ProtFlags::PROT_READ,
                nix::sys::mman::MapFlags::MAP_PRIVATE,
                fd.as_fd(),
                0,
            )
        }
        .map_err(|e| io::Error::other(format!("mmap failed: {e}")))?;

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        unsafe {
            let _ = nix::sys::mman::madvise(ptr, len, nix::sys::mman::MmapAdvise::MADV_SEQUENTIAL);
        }

        Ok(Self {
            ptr: ptr.as_ptr(),
            len,
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        if self.len == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.ptr as *const u8, self.len) }
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

impl AsRef<[u8]> for MappedFile {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Drop for MappedFile {
    fn drop(&mut self) {
        if self.len > 0 {
            let _ = unsafe {
                nix::sys::mman::munmap(
                    std::ptr::NonNull::new(self.ptr).expect("mmap returned null"),
                    self.len,
                )
            };
        }
    }
}
