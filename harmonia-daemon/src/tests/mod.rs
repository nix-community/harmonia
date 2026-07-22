mod add_to_store_scanning;
mod sqlite_nix_store;
mod submit_output;

use std::future::ready;
use std::time::Duration;

use tokio::io::AsyncWriteExt as _;

use harmonia_protocol::ProtocolVersion;
use harmonia_protocol::daemon::{
    DaemonResult, DaemonStore, FutureResultExt as _, HandshakeDaemonStore, ResultLog,
    wire::{CLIENT_MAGIC, SERVER_MAGIC, logger::RawLogMessage},
};
use harmonia_protocol::de::{NixRead as _, NixReader};
use harmonia_protocol::ser::{NixWrite as _, NixWriter};
use harmonia_protocol::types::TrustLevel;
use harmonia_protocol::version::FeatureSet;
use harmonia_utils_io::BytesReader;

use crate::server::Builder;

pub(crate) const TEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Reports every operation as unimplemented.
#[derive(Debug)]
pub(crate) struct NullStore;

impl HandshakeDaemonStore for NullStore {
    type Store = Self;

    fn handshake(self) -> impl ResultLog<Output = DaemonResult<Self::Store>> + Send {
        ready(Ok(self)).empty_logs()
    }
}

impl DaemonStore for NullStore {
    fn trust_level(&self) -> Option<TrustLevel> {
        None
    }

    fn shutdown(&mut self) -> impl std::future::Future<Output = DaemonResult<()>> + Send + '_ {
        ready(Ok(()))
    }
}

pub(crate) fn spawn_server(
    stream: tokio::io::DuplexStream,
) -> tokio::task::JoinHandle<DaemonResult<()>> {
    tokio::spawn(async move {
        let (read, write) = tokio::io::split(stream);
        Builder::new()
            .serve_connection(read, write, NullStore)
            .await
    })
}

pub(crate) type RawReader = NixReader<BytesReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>;
pub(crate) type RawWriter = NixWriter<tokio::io::WriteHalf<tokio::io::DuplexStream>>;

/// Hand-rolled client handshake advertising `local`, returning the daemon's
/// advertised features.
pub(crate) async fn raw_client_handshake(
    stream: tokio::io::DuplexStream,
    local: FeatureSet,
) -> (RawReader, RawWriter, FeatureSet) {
    let (read, write) = tokio::io::split(stream);
    let mut reader = NixReader::builder().build_buffered(read);
    let mut writer = NixWriter::builder().build(write);

    writer.write_number(CLIENT_MAGIC).await.unwrap();
    writer.flush().await.unwrap();
    assert_eq!(reader.read_number().await.unwrap(), SERVER_MAGIC);
    let version: ProtocolVersion = reader.read_value().await.unwrap();
    writer.write_value(&version).await.unwrap();
    reader.set_version(version);
    writer.set_version(version);

    writer.write_value(&local).await.unwrap();
    writer.flush().await.unwrap();
    let daemon_features: FeatureSet = reader.read_value().await.unwrap();

    writer.write_value(&false).await.unwrap(); // obsolete CPU affinity
    writer.write_value(&false).await.unwrap(); // obsolete reserve space
    writer.flush().await.unwrap();
    let _nix_version: String = reader.read_value().await.unwrap();
    let _trust: Option<TrustLevel> = reader.read_value().await.unwrap();

    // Drain the store handshake's stderr stream.
    loop {
        match reader.read_value::<RawLogMessage>().await.unwrap() {
            RawLogMessage::Last => break,
            RawLogMessage::Error(err) => panic!("handshake error: {:?}", err.msg),
            _ => {}
        }
    }

    (reader, writer, daemon_features)
}
