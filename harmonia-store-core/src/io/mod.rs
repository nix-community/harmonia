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
