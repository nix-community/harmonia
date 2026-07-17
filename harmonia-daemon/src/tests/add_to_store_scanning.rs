// SPDX-License-Identifier: MIT

//! Client/server tests for the `AddToStoreScanning` operation.
//!
//! Harmonia's daemon never advertises the `add-to-store-scanning` feature,
//! so both sides must reject the operation gracefully.

use std::future::ready;
use std::io::Cursor;
use std::time::Duration;

use tokio::io::AsyncWriteExt as _;
use tokio::time::timeout;

use harmonia_protocol::ProtocolVersion;
use harmonia_protocol::daemon::{
    DaemonResult, DaemonStore, FutureResultExt as _, HandshakeDaemonStore, ResultLog,
    wire::{CLIENT_MAGIC, SERVER_MAGIC, logger::RawLogMessage, types::Operation},
};
use harmonia_protocol::de::{NixRead as _, NixReader};
use harmonia_protocol::ser::{NixWrite as _, NixWriter};
use harmonia_protocol::types::TrustLevel;
use harmonia_protocol::version::{FEATURE_ADD_TO_STORE_SCANNING, FeatureSet};
use harmonia_store_content_address::ContentAddressMethodAlgorithm;
use harmonia_store_path::StorePath;
use harmonia_store_remote::DaemonClient;
use harmonia_utils_hash::Algorithm;

use crate::server::Builder;

const TEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Reports every operation as unimplemented.
#[derive(Debug)]
struct NullStore;

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

fn spawn_server(stream: tokio::io::DuplexStream) -> tokio::task::JoinHandle<DaemonResult<()>> {
    tokio::spawn(async move {
        let (read, write) = tokio::io::split(stream);
        Builder::new()
            .serve_connection(read, write, NullStore)
            .await
    })
}

/// The client refuses to send the operation without the negotiated feature.
#[tokio::test]
async fn client_requires_negotiated_feature() {
    timeout(TEST_TIMEOUT, async {
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let server = spawn_server(server_side);

        let (read, write) = tokio::io::split(client_side);
        let mut client = DaemonClient::builder()
            .connect(read, write)
            .await
            .expect("client handshake");
        assert!(!client.has_feature(FEATURE_ADD_TO_STORE_SCANNING));

        let cam = ContentAddressMethodAlgorithm::NixArchive(Algorithm::SHA256);
        let err = client
            .add_to_store_scanning("example", cam, Cursor::new(&b""[..]))
            .await
            .expect_err("must fail without the negotiated feature");
        assert!(
            err.to_string().contains(FEATURE_ADD_TO_STORE_SCANNING),
            "unexpected error: {err}"
        );

        drop(client);
        server.await.expect("join").expect("server");
    })
    .await
    .expect("test timed out");
}

/// A client that sends the operation anyway gets a recoverable error and
/// the connection stays usable.
#[tokio::test]
async fn server_rejects_unnegotiated_scanning_op() {
    timeout(TEST_TIMEOUT, async {
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let server = spawn_server(server_side);

        let (read, write) = tokio::io::split(client_side);
        let mut reader = NixReader::builder().build_buffered(read);
        let mut writer = NixWriter::builder().build(write);

        // Handshake, advertising the scanning feature ourselves.
        writer.write_number(CLIENT_MAGIC).await.unwrap();
        writer.flush().await.unwrap();
        assert_eq!(reader.read_number().await.unwrap(), SERVER_MAGIC);
        let version: ProtocolVersion = reader.read_value().await.unwrap();
        writer.write_value(&version).await.unwrap();
        reader.set_version(version);
        writer.set_version(version);

        let local: FeatureSet = [FEATURE_ADD_TO_STORE_SCANNING.to_owned()].into();
        writer.write_value(&local).await.unwrap();
        writer.flush().await.unwrap();
        let daemon_features: FeatureSet = reader.read_value().await.unwrap();
        assert!(
            !daemon_features.contains(FEATURE_ADD_TO_STORE_SCANNING),
            "daemon must not advertise the scanning feature"
        );

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

        // Send the operation despite the failed negotiation.
        let cam = ContentAddressMethodAlgorithm::NixArchive(Algorithm::SHA256);
        writer
            .write_value(&Operation::AddToStoreScanning)
            .await
            .unwrap();
        writer.write_value("example").await.unwrap();
        writer.write_value(&cam).await.unwrap();
        writer.write_number(0).await.unwrap(); // framed stream terminator
        writer.flush().await.unwrap();

        let RawLogMessage::Error(err) = reader.read_value::<RawLogMessage>().await.unwrap() else {
            panic!("expected an error for the unnegotiated operation");
        };
        let msg = String::from_utf8_lossy(&err.msg).into_owned();
        assert!(
            msg.contains("not supported in negotiated protocol"),
            "unexpected error: {msg}"
        );

        // The connection must still be in sync for the next request.
        let path = StorePath::from_bytes(b"00000000000000000000000000000000-x").unwrap();
        writer.write_value(&Operation::IsValidPath).await.unwrap();
        writer.write_value(&path).await.unwrap();
        writer.flush().await.unwrap();

        let RawLogMessage::Error(err) = reader.read_value::<RawLogMessage>().await.unwrap() else {
            panic!("expected NullStore to report IsValidPath as unimplemented");
        };
        let msg = String::from_utf8_lossy(&err.msg).into_owned();
        assert!(msg.contains("IsValidPath"), "unexpected error: {msg}");

        // Both halves must drop or the server never sees EOF.
        drop(reader);
        drop(writer);
        server.await.expect("join").expect("server");
    })
    .await
    .expect("test timed out");
}
