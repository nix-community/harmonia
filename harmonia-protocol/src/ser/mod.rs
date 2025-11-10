use std::error::Error as StdError;
use std::future::Future;
use std::{fmt, io};

use crate::store_path::StoreDir;

use super::ProtocolVersion;

mod bytes;
mod collections;
mod int;
#[cfg(any(test, feature = "test"))]
pub mod mock;
mod writer;

pub use writer::{NixWriter, NixWriterBuilder};

pub trait Error: Sized + StdError {
    fn custom<T: fmt::Display>(msg: T) -> Self;

    #[allow(unused_variables)]
    fn with_field(self, field: &'static str) -> Self {
        self
    }

    fn io_error(err: std::io::Error) -> Self {
        Self::custom(format_args!("There was an I/O error {err}"))
    }

    fn unsupported_data<T: fmt::Display>(msg: T) -> Self {
        Self::custom(msg)
    }

    fn invalid_enum<T: fmt::Display>(msg: T) -> Self {
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

    fn unsupported_data<T: fmt::Display>(msg: T) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
    }
}

pub trait NixWrite: Send {
    type Error: Error;

    /// Some types are serialized differently depending on the version
    /// of the protocol and so this can be used for implementing that.
    fn version(&self) -> ProtocolVersion;
    fn store_dir(&self) -> &StoreDir;

    fn write_number(&mut self, value: u64) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn write_slice(&mut self, buf: &[u8]) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn write_display<D>(&mut self, msg: D) -> impl Future<Output = Result<(), Self::Error>> + Send
    where
        D: fmt::Display + Send,
        Self: Sized,
    {
        async move {
            let s = msg.to_string();
            self.write_slice(s.as_bytes()).await
        }
    }

    /// Write a value to the protocol.
    /// Uses `NixSerialize::serialize` to write the value.
    fn write_value<V>(&mut self, value: &V) -> impl Future<Output = Result<(), Self::Error>> + Send
    where
        V: NixSerialize + Send + ?Sized,
        Self: Sized,
    {
        value.serialize(self)
    }
}

impl<T: NixWrite> NixWrite for &mut T {
    type Error = T::Error;

    fn version(&self) -> ProtocolVersion {
        (**self).version()
    }

    fn store_dir(&self) -> &StoreDir {
        (**self).store_dir()
    }

    fn write_number(&mut self, value: u64) -> impl Future<Output = Result<(), Self::Error>> + Send {
        (**self).write_number(value)
    }

    fn write_slice(&mut self, buf: &[u8]) -> impl Future<Output = Result<(), Self::Error>> + Send {
        (**self).write_slice(buf)
    }

    fn write_display<D>(&mut self, msg: D) -> impl Future<Output = Result<(), Self::Error>> + Send
    where
        D: fmt::Display + Send,
        Self: Sized,
    {
        (**self).write_display(msg)
    }

    fn write_value<V>(&mut self, value: &V) -> impl Future<Output = Result<(), Self::Error>> + Send
    where
        V: NixSerialize + Send + ?Sized,
        Self: Sized,
    {
        (**self).write_value(value)
    }
}

pub trait NixSerialize {
    fn serialize<W>(&self, writer: &mut W) -> impl Future<Output = Result<(), W::Error>> + Send
    where
        W: NixWrite;
}
