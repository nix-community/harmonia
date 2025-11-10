use crate::{NixDeserialize, NixSerialize};
#[cfg(test)]
use proptest_derive::Arbitrary;

pub const CLIENT_MAGIC: u64 = 0x6e697863; // 'nixc' in ASCII
pub const SERVER_MAGIC: u64 = 0x6478696f; // 'dxio' in ASCII

mod add_multiple_to_store;
mod framed;
pub mod logger;
mod stderr_read;
pub mod types;
pub mod types2;

pub use add_multiple_to_store::{
    SizedStream, parse_add_multiple_to_store, write_add_multiple_to_store_stream,
};
pub use framed::reader::FramedReader;
pub use framed::writer::FramedWriter;
pub use stderr_read::StderrReader;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, NixDeserialize, NixSerialize)]
#[cfg_attr(test, derive(Arbitrary))]
#[nix(from = "u64", into = "u64")]
pub struct IgnoredZero;
impl From<u64> for IgnoredZero {
    fn from(_: u64) -> Self {
        IgnoredZero
    }
}

impl From<IgnoredZero> for u64 {
    fn from(_: IgnoredZero) -> Self {
        0
    }
}

#[derive(Clone, Debug, NixDeserialize, NixSerialize)]
#[cfg_attr(test, derive(Arbitrary))]
#[nix(from = "u64", into = "u64")]
pub struct IgnoredOne;
impl From<u64> for IgnoredOne {
    fn from(_: u64) -> Self {
        IgnoredOne
    }
}

impl From<IgnoredOne> for u64 {
    fn from(_: IgnoredOne) -> Self {
        1
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, NixDeserialize, NixSerialize)]
#[cfg_attr(test, derive(Arbitrary))]
#[nix(from = "bool", into = "bool")]
pub struct IgnoredTrue;
impl From<bool> for IgnoredTrue {
    fn from(_: bool) -> Self {
        IgnoredTrue
    }
}

impl From<IgnoredTrue> for bool {
    fn from(_: IgnoredTrue) -> Self {
        true
    }
}
