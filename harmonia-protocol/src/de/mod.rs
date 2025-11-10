use std::error::Error as StdError;
use std::future::Future;
use std::ops::RangeInclusive;
use std::{fmt, io};

use ::bytes::Bytes;

use crate::store_path::StoreDir;

use super::ProtocolVersion;

mod bytes;
mod collections;
mod int;
#[cfg(any(test, feature = "test"))]
pub mod mock;
mod reader;

pub use reader::{NixReader, NixReaderBuilder};

pub trait Error: Sized + StdError {
    fn custom<T: fmt::Display>(msg: T) -> Self;

    #[allow(unused_variables)]
    fn with_field(self, field: &'static str) -> Self {
        self
    }

    fn io_error(err: std::io::Error) -> Self {
        Self::custom(format_args!("There was an I/O error {err}"))
    }

    fn invalid_data<T: fmt::Display>(msg: T) -> Self {
        Self::custom(msg)
    }

    fn missing_data<T: fmt::Display>(msg: T) -> Self {
        Self::custom(msg)
    }
}

impl Error for io::Error {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        io::Error::other(msg.to_string())
    }

    fn io_error(err: std::io::Error) -> Self {
        err
    }

    fn invalid_data<T: fmt::Display>(msg: T) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
    }

    fn missing_data<T: fmt::Display>(msg: T) -> Self {
        io::Error::new(io::ErrorKind::UnexpectedEof, msg.to_string())
    }
}

/// A reader of data from the Nix daemon protocol.
/// Basically there are two basic types in the Nix daemon protocol
/// u64 and a bytes buffer. Everything else is more or less built on
/// top of these two types.
pub trait NixRead: Send {
    type Error: Error + Send;

    /// Some types are serialized differently depending on the version
    /// of the protocol and so this can be used for implementing that.
    fn version(&self) -> ProtocolVersion;
    fn store_dir(&self) -> &StoreDir;

    /// Read a single u64 from the protocol.
    /// This returns an Option to support gracefull shutdown.
    fn try_read_number(
        &mut self,
    ) -> impl Future<Output = Result<Option<u64>, Self::Error>> + Send + '_;

    /// Read bytes from the protocol.
    /// You also specify a limit to have a limit on how long the returned
    /// bytes must be within.
    /// This returns an Option to support gracefull shutdown.
    fn try_read_bytes_limited(
        &mut self,
        limit: RangeInclusive<usize>,
    ) -> impl Future<Output = Result<Option<Bytes>, Self::Error>> + Send + '_;

    /// Read bytes from the protocol without a limit.
    /// The default implementation just calls `try_read_bytes_limited` with a
    /// limit of `0..=usize::MAX` but other implementations are free to have a
    /// reader wide limit.
    /// This returns an Option to support gracefull shutdown.
    fn try_read_bytes(
        &mut self,
    ) -> impl Future<Output = Result<Option<Bytes>, Self::Error>> + Send + '_ {
        self.try_read_bytes_limited(0..=usize::MAX)
    }

    /// Read bytes from the protocol without a limit.
    /// This will return an error if the number could not be read.
    fn read_number(&mut self) -> impl Future<Output = Result<u64, Self::Error>> + Send + '_ {
        async move {
            match self.try_read_number().await? {
                Some(v) => Ok(v),
                None => Err(Self::Error::missing_data("unexpected end-of-file")),
            }
        }
    }

    /// Read bytes from the protocol.
    /// You also specify a limit to have a limit on how long the returned
    /// bytes must be within.
    /// This will return an error if the number could not be read.
    fn read_bytes_limited(
        &mut self,
        limit: RangeInclusive<usize>,
    ) -> impl Future<Output = Result<Bytes, Self::Error>> + Send + '_ {
        async move {
            match self.try_read_bytes_limited(limit).await? {
                Some(v) => Ok(v),
                None => Err(Self::Error::missing_data("unexpected end-of-file")),
            }
        }
    }

    /// Read bytes from the protocol.
    /// The default implementation just calls `read_bytes_limited` with a
    /// limit of `0..=usize::MAX` but other implementations are free to have a
    /// reader wide limit.
    /// This will return an error if the bytes could not be read.
    fn read_bytes(&mut self) -> impl Future<Output = Result<Bytes, Self::Error>> + Send + '_ {
        self.read_bytes_limited(0..=usize::MAX)
    }

    /// Read a value from the protocol.
    /// Uses `NixDeserialize::deserialize` to read a value.
    fn read_value<V: NixDeserialize>(
        &mut self,
    ) -> impl Future<Output = Result<V, Self::Error>> + Send + '_ {
        V::deserialize(self)
    }

    /// Read a value from the protocol.
    /// Uses `NixDeserialize::try_deserialize` to read a value.
    /// This returns an Option to support gracefull shutdown.
    fn try_read_value<V: NixDeserialize>(
        &mut self,
    ) -> impl Future<Output = Result<Option<V>, Self::Error>> + Send + '_ {
        V::try_deserialize(self)
    }
}

impl<T: ?Sized + NixRead> NixRead for &mut T {
    type Error = T::Error;

    fn version(&self) -> ProtocolVersion {
        (**self).version()
    }

    fn store_dir(&self) -> &StoreDir {
        (**self).store_dir()
    }

    fn try_read_number(
        &mut self,
    ) -> impl Future<Output = Result<Option<u64>, Self::Error>> + Send + '_ {
        (**self).try_read_number()
    }

    fn try_read_bytes_limited(
        &mut self,
        limit: RangeInclusive<usize>,
    ) -> impl Future<Output = Result<Option<Bytes>, Self::Error>> + Send + '_ {
        (**self).try_read_bytes_limited(limit)
    }

    fn try_read_bytes(
        &mut self,
    ) -> impl Future<Output = Result<Option<Bytes>, Self::Error>> + Send + '_ {
        (**self).try_read_bytes()
    }

    fn read_number(&mut self) -> impl Future<Output = Result<u64, Self::Error>> + Send + '_ {
        (**self).read_number()
    }

    fn read_bytes_limited(
        &mut self,
        limit: RangeInclusive<usize>,
    ) -> impl Future<Output = Result<Bytes, Self::Error>> + Send + '_ {
        (**self).read_bytes_limited(limit)
    }

    fn read_bytes(&mut self) -> impl Future<Output = Result<Bytes, Self::Error>> + Send + '_ {
        (**self).read_bytes()
    }

    fn try_read_value<V: NixDeserialize>(
        &mut self,
    ) -> impl Future<Output = Result<Option<V>, Self::Error>> + Send + '_ {
        (**self).try_read_value()
    }

    fn read_value<V: NixDeserialize>(
        &mut self,
    ) -> impl Future<Output = Result<V, Self::Error>> + Send + '_ {
        (**self).read_value()
    }
}

/// A data structure that can be deserialized from the Nix daemon
/// worker protocol.
pub trait NixDeserialize: Sized {
    /// Read a value from the reader.
    /// This returns an Option to support gracefull shutdown.
    fn try_deserialize<R>(
        reader: &mut R,
    ) -> impl Future<Output = Result<Option<Self>, R::Error>> + Send + '_
    where
        R: ?Sized + NixRead + Send;

    fn deserialize<R>(reader: &mut R) -> impl Future<Output = Result<Self, R::Error>> + Send + '_
    where
        R: ?Sized + NixRead + Send,
    {
        async move {
            match Self::try_deserialize(reader).await? {
                Some(v) => Ok(v),
                None => Err(R::Error::missing_data("unexpected end-of-file")),
            }
        }
    }
}
