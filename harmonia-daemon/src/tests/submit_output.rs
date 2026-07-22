// SPDX-License-Identifier: MIT

//! Client/server tests for the `SubmitOutput` operation.
//!
//! Harmonia's daemon never advertises the `submit-output` feature, so both
//! sides must reject the operation gracefully.

use std::future::ready;
use std::sync::Arc;

use tokio::io::AsyncWriteExt as _;
use tokio::time::timeout;

use harmonia_protocol::daemon::{
    DaemonResult, DaemonStore, FutureResultExt as _, HandshakeDaemonStore, ResultLog,
    wire::{logger::RawLogMessage, types::Operation},
};
use harmonia_protocol::de::NixRead as _;
use harmonia_protocol::ser::NixWrite as _;
use harmonia_protocol::types::TrustLevel;
use harmonia_protocol::version::{FEATURE_SUBMIT_OUTPUT, FeatureSet};
use harmonia_store_derivation::derived_path::{OutputName, SingleDerivedPath};
use harmonia_store_path::StorePath;
use harmonia_store_remote::DaemonClient;

use crate::server::Builder;

use super::{TEST_TIMEOUT, raw_client_handshake, spawn_server};

fn out() -> OutputName {
    "out".parse().unwrap()
}

/// Accepts `SubmitOutput` and reports everything else as unimplemented.
#[derive(Debug)]
struct AcceptingStore;

impl HandshakeDaemonStore for AcceptingStore {
    type Store = Self;

    fn handshake(self) -> impl ResultLog<Output = DaemonResult<Self::Store>> + Send {
        ready(Ok(self)).empty_logs()
    }
}

impl DaemonStore for AcceptingStore {
    fn trust_level(&self) -> Option<TrustLevel> {
        None
    }

    fn submit_output<'a>(
        &'a mut self,
        _path: &'a SingleDerivedPath,
        _output: &'a OutputName,
    ) -> impl ResultLog<Output = DaemonResult<()>> + Send + 'a {
        ready(Ok(())).empty_logs()
    }

    fn shutdown(&mut self) -> impl std::future::Future<Output = DaemonResult<()>> + Send + '_ {
        ready(Ok(()))
    }
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
        assert!(!client.has_feature(FEATURE_SUBMIT_OUTPUT));

        let path = SingleDerivedPath::Opaque(
            StorePath::from_bytes(b"00000000000000000000000000000000-x").unwrap(),
        );
        let err = client
            .submit_output(&path, &out())
            .await
            .expect_err("must fail without the negotiated feature");
        assert!(
            err.to_string().contains(FEATURE_SUBMIT_OUTPUT),
            "unexpected error: {err}"
        );

        drop(client);
        server.await.expect("join").expect("server");
    })
    .await
    .expect("test timed out");
}

/// A store that implements the operation answers with the log terminator and
/// the trailing `1`, leaving the connection in sync.
#[tokio::test]
async fn server_accepts_submit_output() {
    timeout(TEST_TIMEOUT, async {
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let (read, write) = tokio::io::split(server_side);
            Builder::new()
                .serve_connection(read, write, AcceptingStore)
                .await
        });

        let local: FeatureSet = [FEATURE_SUBMIT_OUTPUT.to_owned()].into();
        let (mut reader, mut writer, _daemon_features) =
            raw_client_handshake(client_side, local).await;

        let path = SingleDerivedPath::Opaque(
            StorePath::from_bytes(b"00000000000000000000000000000000-x").unwrap(),
        );
        writer.write_value(&Operation::SubmitOutput).await.unwrap();
        writer.write_value(&path).await.unwrap();
        writer.write_value(&out()).await.unwrap();
        writer.flush().await.unwrap();

        loop {
            match reader.read_value::<RawLogMessage>().await.unwrap() {
                RawLogMessage::Last => break,
                RawLogMessage::Error(err) => panic!("unexpected error: {:?}", err.msg),
                _ => {}
            }
        }
        assert_eq!(reader.read_value::<u64>().await.unwrap(), 1);

        // Both halves must drop or the server never sees EOF.
        drop(reader);
        drop(writer);
        server.await.expect("join").expect("server");
    })
    .await
    .expect("test timed out");
}

/// A client that sends the operation anyway gets a recoverable error and
/// the connection stays usable.
#[tokio::test]
async fn server_rejects_unnegotiated_submit_output() {
    timeout(TEST_TIMEOUT, async {
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let server = spawn_server(server_side);

        let local: FeatureSet = [FEATURE_SUBMIT_OUTPUT.to_owned()].into();
        let (mut reader, mut writer, daemon_features) =
            raw_client_handshake(client_side, local).await;
        assert!(
            !daemon_features.contains(FEATURE_SUBMIT_OUTPUT),
            "daemon must not advertise the submit-output feature"
        );

        // Send the operation despite the failed negotiation, with a nested
        // built path to exercise the tagged encoding.
        let path = SingleDerivedPath::Built {
            drv_path: Arc::new(SingleDerivedPath::Opaque(
                StorePath::from_bytes(b"00000000000000000000000000000000-x.drv").unwrap(),
            )),
            output: out(),
        };
        writer.write_value(&Operation::SubmitOutput).await.unwrap();
        writer.write_value(&path).await.unwrap();
        writer.write_value(&out()).await.unwrap();
        writer.flush().await.unwrap();

        let RawLogMessage::Error(err) = reader.read_value::<RawLogMessage>().await.unwrap() else {
            panic!("expected an error for the unnegotiated operation");
        };
        let msg = String::from_utf8_lossy(&err.msg).into_owned();
        assert!(msg.contains("SubmitOutput"), "unexpected error: {msg}");

        // The connection must still be in sync for the next request.
        let store_path = StorePath::from_bytes(b"00000000000000000000000000000000-x").unwrap();
        writer.write_value(&Operation::IsValidPath).await.unwrap();
        writer.write_value(&store_path).await.unwrap();
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
