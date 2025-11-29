// SPDX-FileCopyrightText: 2024 griff (original Nix.rs)
// SPDX-FileCopyrightText: 2025 Jörg Thalheim (Harmonia adaptation)
// SPDX-License-Identifier: EUPL-1.2 OR MIT
//
// This crate is derived from Nix.rs (https://github.com/griff/Nix.rs)
// Upstream commit: f5d129b71bb30b476ce21e6da2a53dcb28607a89

//! Async I/O utilities for Harmonia.

mod async_bytes_read;
mod bytes_reader;
mod compat;
mod lending;
mod read_u64;
mod tee;
mod try_read_bytes_limited;

pub use async_bytes_read::AsyncBytesRead;
pub use bytes_reader::{BytesReader, DEFAULT_MAX_BUF_SIZE, DEFAULT_RESERVED_BUF_SIZE};
pub use compat::AsyncBufReadCompat;
pub use lending::{DrainInto, Lending, LentReader};
pub use read_u64::TryReadU64;
pub use tee::TeeWriter;
pub use try_read_bytes_limited::TryReadBytesLimited;

pub const DEFAULT_BUF_SIZE: usize = 32 * 1024;
pub const RESERVED_BUF_SIZE: usize = DEFAULT_BUF_SIZE / 2;

/// Wire protocol utilities.
pub mod wire {
    /// Zero bytes for padding.
    pub const ZEROS: [u8; 8] = [0u8; 8];

    pub const fn calc_aligned(len: u64) -> u64 {
        len.wrapping_add(7) & !7
    }

    pub const fn calc_padding(len: u64) -> usize {
        let aligned = calc_aligned(len);
        aligned.wrapping_sub(len) as usize
    }
}
