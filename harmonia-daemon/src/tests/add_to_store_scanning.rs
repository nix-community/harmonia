// SPDX-License-Identifier: MIT

//! Client/server tests for the `AddToStoreScanning` operation.
//!
//! Harmonia's daemon never advertises the `add-to-store-scanning` feature,
//! so both sides must reject the operation gracefully.

use std::io::Cursor;

use tokio::io::AsyncWriteExt as _;
use tokio::time::timeout;

use harmonia_protocol::daemon::{
    DaemonStore,
    wire::{logger::RawLogMessage, types::Operation},
};
use harmonia_protocol::de::NixRead as _;
use harmonia_protocol::ser::NixWrite as _;
use harmonia_protocol::version::{FEATURE_ADD_TO_STORE_SCANNING, FeatureSet};
use harmonia_store_content_address::ContentAddressMethodAlgorithm;
use harmonia_store_path::StorePath;
use harmonia_store_remote::DaemonClient;
use harmonia_utils_hash::Algorithm;

use super::{TEST_TIMEOUT, raw_client_handshake, spawn_server};

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

        let local: FeatureSet = [FEATURE_ADD_TO_STORE_SCANNING.to_owned()].into();
        let (mut reader, mut writer, daemon_features) =
            raw_client_handshake(client_side, local).await;
        assert!(
            !daemon_features.contains(FEATURE_ADD_TO_STORE_SCANNING),
            "daemon must not advertise the scanning feature"
        );

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
