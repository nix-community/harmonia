use crate::client::{DaemonClient, PoolConfig};
use crate::error::ProtocolError;
use crate::protocol::{StorePath, ValidPathInfo, CURRENT_PROTOCOL_VERSION};
use crate::serialization::{Deserialize, Serialize};
use crate::server::{DaemonServer, RequestHandler};
use harmonia_store_core::Hash;
use std::collections::{BTreeSet, HashMap};
use std::io::Cursor;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tempfile::{tempdir, TempDir};
use tokio::task::JoinHandle;
use tokio::time::sleep;

const SOCKET_PATH: &str = "/nix/var/nix/daemon-socket/socket";

#[tokio::test]
async fn test_serialization_roundtrip() {
    // Test u64
    let num: u64 = 42;
    let mut buf = Vec::new();
    num.serialize(&mut buf, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();
    let mut cursor = Cursor::new(buf);
    let deserialized = u64::deserialize(&mut cursor, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();
    assert_eq!(num, deserialized);

    // Test string
    let s = "hello world".to_string();
    let mut buf = Vec::new();
    s.serialize(&mut buf, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();
    let mut cursor = Cursor::new(buf);
    let deserialized = String::deserialize(&mut cursor, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();
    assert_eq!(s, deserialized);

    // Test string padding
    let s = "test"; // 4 bytes, needs 4 bytes padding
    let mut buf = Vec::new();
    s.to_string()
        .serialize(&mut buf, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();
    assert_eq!(buf.len(), 8 + 8); // 8 bytes for length + 8 bytes for padded string

    // Test bool
    let b = true;
    let mut buf = Vec::new();
    b.serialize(&mut buf, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();
    let mut cursor = Cursor::new(buf);
    let deserialized = bool::deserialize(&mut cursor, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();
    assert_eq!(b, deserialized);

    // Test Option<String>
    let opt: Option<String> = Some("test".to_string());
    let mut buf = Vec::new();
    opt.serialize(&mut buf, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();
    let mut cursor = Cursor::new(buf);
    let deserialized = Option::<String>::deserialize(&mut cursor, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();
    assert_eq!(opt, deserialized);

    // Test None
    let opt: Option<String> = None;
    let mut buf = Vec::new();
    opt.serialize(&mut buf, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();
    let mut cursor = Cursor::new(buf);
    let deserialized = Option::<String>::deserialize(&mut cursor, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();
    assert_eq!(opt, deserialized);

    // Test Vec<Vec<u8>>
    let vec = vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()];
    let mut buf = Vec::new();
    vec.serialize(&mut buf, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();
    let mut cursor = Cursor::new(buf);
    let deserialized =
        <Vec<Vec<u8>> as Deserialize>::deserialize(&mut cursor, CURRENT_PROTOCOL_VERSION)
            .await
            .unwrap();
    assert_eq!(vec, deserialized);
}

#[tokio::test]
async fn test_valid_path_info_serialization() {
    let info = ValidPathInfo {
        deriver: Some(StorePath::from(b"/nix/store/abc-test.drv".to_vec())),
        hash: Hash::parse(
            b"sha256:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        )
        .unwrap(),
        references: {
            let mut refs = BTreeSet::new();
            refs.insert(StorePath::from(b"/nix/store/ref1".to_vec()));
            refs.insert(StorePath::from(b"/nix/store/ref2".to_vec()));
            refs
        },
        registration_time: 1234567890,
        nar_size: 9876,
        ultimate: true,
        signatures: vec![b"sig1".to_vec(), b"sig2".to_vec()],
        content_address: Some(b"fixed:sha256:xyz".to_vec()),
    };

    let mut buf = Vec::new();
    info.serialize(&mut buf, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();
    let mut cursor = Cursor::new(buf);
    let deserialized = ValidPathInfo::deserialize(&mut cursor, CURRENT_PROTOCOL_VERSION)
        .await
        .unwrap();

    assert_eq!(info, deserialized);
}

async fn test_daemon_operations(socket_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let client = DaemonClient::connect(socket_path).await?;

    // Create a test file and add it to the store
    let temp_dir = TempDir::new()?;
    let temp_file = temp_dir.path().join("test.txt");
    std::fs::write(&temp_file, b"hello from harmonia-store-remote")?;

    let output = Command::new("nix-store")
        .arg("--add")
        .arg(&temp_file)
        .output()?;

    if !output.status.success() {
        return Err(format!(
            "nix-store --add failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }

    let store_path = String::from_utf8(output.stdout)?.trim().to_string();
    let store_path = StorePath::from(store_path);

    // Test is_valid_path
    let is_valid = client.is_valid_path(&store_path).await?;
    assert!(is_valid);

    // Test query_path_info
    let path_info = client.query_path_info(&store_path).await?;
    assert!(path_info.is_some());
    let path_info = path_info.unwrap();
    assert!(path_info.nar_size > 0);
    assert!(!path_info.hash.digest.is_empty());

    // Test query_path_from_hash_part
    let hash_part = store_path
        .to_string()
        .strip_prefix("/nix/store/")
        .ok_or("Invalid store path format")?
        .chars()
        .take(32)
        .collect::<String>();

    let found_path = client
        .query_path_from_hash_part(hash_part.as_bytes())
        .await?;
    assert_eq!(found_path, Some(store_path));

    // Test with non-existent hash
    let not_found = client
        .query_path_from_hash_part(b"zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz")
        .await?;
    assert_eq!(not_found, None);

    // Test is_valid_path with non-existent path
    let invalid_path = StorePath::from("/nix/store/zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-fake");
    let is_valid = client.is_valid_path(&invalid_path).await?;
    assert!(!is_valid);

    Ok(())
}

#[tokio::test]
async fn test_daemon_integration() -> Result<(), Box<dyn std::error::Error>> {
    if !Path::new(SOCKET_PATH).exists() {
        eprintln!("Skipping test: nix-daemon socket not found");
        return Ok(());
    }

    test_daemon_operations(Path::new(SOCKET_PATH)).await
}

async fn wait_for_daemon_server(
    socket_path: &Path,
    timeout: std::time::Duration,
) -> Result<DaemonClient, Box<dyn std::error::Error>> {
    let start = std::time::Instant::now();
    let retry_interval = Duration::from_millis(10);

    loop {
        match DaemonClient::connect(socket_path).await {
            Ok(client) => return Ok(client),
            Err(e) => {
                if start.elapsed() > timeout {
                    return Err(
                        format!("Failed to connect to daemon after {timeout:?}: {e}").into(),
                    );
                }
                sleep(retry_interval).await;
            }
        }
    }
}

#[tokio::test]
async fn test_custom_daemon_server() -> Result<(), Box<dyn std::error::Error>> {
    // Create a test handler with some mock data
    #[derive(Clone)]
    struct TestHandler {
        store_paths: HashMap<Vec<u8>, ValidPathInfo>,
        hash_to_path: HashMap<Vec<u8>, StorePath>,
    }

    impl TestHandler {
        fn new() -> Self {
            let mut handler = Self {
                store_paths: HashMap::new(),
                hash_to_path: HashMap::new(),
            };

            // Add some test data
            let test_path =
                StorePath::from("/nix/store/abc123def456ghi789jkl012mno345p-hello-2.12.1");
            let test_info = ValidPathInfo {
                deriver: Some(StorePath::from("/nix/store/xyz789abc123def456ghi789jkl012m-hello-2.12.1.drv")),
                hash: Hash::parse(b"sha256:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef").unwrap(),
                references: {
                    let mut refs = BTreeSet::new();
                    refs.insert(StorePath::from(b"/nix/store/111111111111111111111111111111111-glibc-2.38".to_vec()));
                    refs.insert(StorePath::from(b"/nix/store/222222222222222222222222222222222-gcc-13.2.0-lib".to_vec()));
                    refs
                },
                registration_time: 1700000000,
                nar_size: 123456,
                ultimate: false,
                signatures: vec![
                    b"cache.nixos.org-1:signature123abc".to_vec(),
                    b"test-cache-1:testsignature456def".to_vec(),
                ],
                content_address: Some(
                    b"fixed:sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".to_vec(),
                ),
            };

            handler
                .store_paths
                .insert(test_path.as_bytes().to_vec(), test_info);
            handler.hash_to_path.insert(
                b"abc123def456ghi789jkl012mno345p".to_vec(),
                test_path.clone(),
            );

            // Add another test path
            let bash_path = StorePath::from(
                b"/nix/store/qrs456tuv789wxy012abc345def678g-bash-5.2-p21".to_vec(),
            );
            let bash_info = ValidPathInfo {
                deriver: Some(StorePath::from(
                    b"/nix/store/mno345pqr678stu901vwx234yz567ab-bash-5.2-p21.drv".to_vec(),
                )),
                hash: Hash::parse(
                    b"sha256:fedcba0987654321fedcba0987654321fedcba0987654321fedcba0987654321",
                )
                .unwrap(),
                references: {
                    let mut refs = BTreeSet::new();
                    refs.insert(StorePath::from(
                        b"/nix/store/111111111111111111111111111111111-glibc-2.38".to_vec(),
                    ));
                    refs.insert(StorePath::from(
                        b"/nix/store/333333333333333333333333333333333-readline-8.2p7".to_vec(),
                    ));
                    refs.insert(StorePath::from(
                        b"/nix/store/444444444444444444444444444444444-ncurses-6.4".to_vec(),
                    ));
                    refs
                },
                registration_time: 1700000100,
                nar_size: 987654,
                ultimate: true,
                signatures: vec![b"cache.nixos.org-1:bashsignature789xyz".to_vec()],
                content_address: None,
            };

            handler
                .store_paths
                .insert(bash_path.as_bytes().to_vec(), bash_info);
            handler
                .hash_to_path
                .insert(b"qrs456tuv789wxy012abc345def678g".to_vec(), bash_path);

            handler
        }
    }

    impl RequestHandler for TestHandler {
        async fn handle_query_path_info(
            &self,
            path: &StorePath,
        ) -> Result<Option<ValidPathInfo>, ProtocolError> {
            Ok(self.store_paths.get(path.as_bytes()).cloned())
        }

        async fn handle_query_path_from_hash_part(
            &self,
            hash: &[u8],
        ) -> Result<Option<StorePath>, ProtocolError> {
            // Support partial hash matching like real nix-daemon
            for (full_hash, path) in &self.hash_to_path {
                if full_hash.starts_with(hash) {
                    return Ok(Some(path.clone()));
                }
            }
            Ok(None)
        }

        async fn handle_is_valid_path(&self, path: &StorePath) -> Result<bool, ProtocolError> {
            Ok(self.store_paths.contains_key(path.as_bytes()))
        }
    }

    // Create a temporary directory for the socket
    let temp_dir = tempdir()?;
    let socket_path = temp_dir.path().join("test-daemon.socket");

    // Spawn the server
    let server = DaemonServer::new(TestHandler::new(), socket_path.clone());
    let server_handle = tokio::spawn(async move {
        let _ = server.serve().await;
    });

    // Wait for server to be ready
    let client = wait_for_daemon_server(&socket_path, Duration::from_secs(5)).await?;

    // Test valid path operations
    let hello_path =
        StorePath::from(b"/nix/store/abc123def456ghi789jkl012mno345p-hello-2.12.1".to_vec());
    assert!(client.is_valid_path(&hello_path).await?);

    let path_info = client.query_path_info(&hello_path).await?;
    assert!(path_info.is_some());
    let info = path_info.unwrap();
    assert_eq!(info.nar_size, 123456);
    assert_eq!(info.references.len(), 2);
    assert_eq!(info.signatures.len(), 2);
    assert!(info.content_address.is_some());

    // Test hash part lookup
    let found_path = client.query_path_from_hash_part(b"abc123").await?;
    assert_eq!(found_path, Some(hello_path.clone()));

    // Test partial hash matching
    let bash_path = client.query_path_from_hash_part(b"qrs456").await?;
    assert!(bash_path.is_some());
    assert!(bash_path.unwrap().to_string().contains("bash"));

    // Test with non-existent data
    let fake_path = StorePath::from(b"/nix/store/zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz-fake".to_vec());
    assert!(!client.is_valid_path(&fake_path).await?);
    assert_eq!(client.query_path_info(&fake_path).await?, None);
    assert_eq!(client.query_path_from_hash_part(b"zzzzz").await?, None);

    // Cleanup: abort the server task
    server_handle.abort();

    Ok(())
}

#[tokio::test]
async fn test_connection_retry_with_server_restart() -> Result<(), Box<dyn std::error::Error>> {
    dbg!("Starting test");

    // Create a test handler that tracks requests
    #[derive(Clone)]
    struct TestHandler {
        id: String,
        store_paths: HashMap<Vec<u8>, ValidPathInfo>,
        request_count: Arc<StdMutex<u32>>,
    }

    impl TestHandler {
        fn new(id: &str) -> Self {
            let mut handler = Self {
                id: id.to_string(),
                store_paths: HashMap::new(),
                request_count: Arc::new(StdMutex::new(0)),
            };

            // Add test data
            let test_path = StorePath::from(
                b"/nix/store/test123abc456def789ghi012jkl345m-test-package".to_vec(),
            );
            let test_info = ValidPathInfo {
                deriver: None,
                hash: Hash::parse(
                    b"sha256:1111111111111111111111111111111111111111111111111111111111111111",
                )
                .unwrap(),
                references: BTreeSet::new(),
                registration_time: 1700000000,
                nar_size: 42,
                ultimate: true,
                signatures: vec![],
                content_address: None,
            };

            handler
                .store_paths
                .insert(test_path.as_bytes().to_vec(), test_info);

            handler
        }
    }

    impl RequestHandler for TestHandler {
        async fn handle_query_path_info(
            &self,
            path: &StorePath,
        ) -> Result<Option<ValidPathInfo>, ProtocolError> {
            let count = {
                let mut count = self.request_count.lock().unwrap();
                *count += 1;
                *count
            };
            println!(
                "TestHandler[{}]::handle_query_path_info called, count={}",
                self.id, count
            );
            Ok(self.store_paths.get(path.as_bytes()).cloned())
        }

        async fn handle_query_path_from_hash_part(
            &self,
            _hash: &[u8],
        ) -> Result<Option<StorePath>, ProtocolError> {
            let count = {
                let mut count = self.request_count.lock().unwrap();
                *count += 1;
                *count
            };
            println!(
                "TestHandler[{}]::handle_query_path_from_hash_part called, count={}",
                self.id, count
            );
            Ok(None)
        }

        async fn handle_is_valid_path(&self, path: &StorePath) -> Result<bool, ProtocolError> {
            let count = {
                let mut count = self.request_count.lock().unwrap();
                *count += 1;
                *count
            };
            println!(
                "TestHandler[{}]::handle_is_valid_path called, count={}",
                self.id, count
            );
            Ok(self.store_paths.contains_key(path.as_bytes()))
        }
    }

    // Helper function to start a server
    async fn start_server(
        handler: TestHandler,
        socket_path: &Path,
    ) -> (Arc<DaemonServer<TestHandler>>, JoinHandle<()>) {
        let server = Arc::new(DaemonServer::new(handler, socket_path.to_path_buf()));
        let server_clone = server.clone();
        let handle = tokio::spawn(async move {
            let _ = server_clone.serve().await;
        });
        (server, handle)
    }

    // Create a temporary directory for the socket
    dbg!("Creating temp dir");
    let temp_dir = tempdir().map_err(|e| format!("Failed to create temp dir: {e}"))?;
    dbg!("Temp dir created");
    let socket_path = temp_dir.path().join("test-daemon-retry.socket");
    dbg!(&socket_path);

    // Start the first server
    dbg!("Creating handler1");
    let handler1 = TestHandler::new("server1");
    let request_count_ref = handler1.request_count.clone();
    dbg!("Starting server1");
    let (server1, server_handle1) = start_server(handler1, &socket_path).await;
    dbg!("Server1 started");

    // Create client with custom pool config
    let pool_config = PoolConfig {
        max_size: 3,                               // Allow 3 connections in pool
        max_idle_time: Duration::from_millis(100), // Short idle time
        connection_timeout: Duration::from_millis(500),
        metrics: None,
    };
    dbg!("Connecting client");
    let client = DaemonClient::connect_with_config(&socket_path, pool_config)
        .await
        .map_err(|e| format!("Failed to connect client: {e:?}"))?;
    dbg!("Client connected");

    // Make some requests to establish connections in the pool
    let test_path =
        StorePath::from(b"/nix/store/test123abc456def789ghi012jkl345m-test-package".to_vec());

    // Make some concurrent requests to exercise the connection pool
    let client1 = client.clone();
    let client2 = client.clone();
    let client3 = client.clone();
    let path1 = test_path.clone();
    let path2 = test_path.clone();
    let path3 = test_path.clone();

    // Launch concurrent requests
    let (r1, r2, r3) = tokio::join!(
        async { client1.is_valid_path(&path1).await },
        async { client2.query_path_info(&path2).await },
        async { client3.is_valid_path(&path3).await }
    );

    assert!(r1.map_err(|e| format!("First is_valid_path failed: {e:?}"))?);
    assert!(r2
        .map_err(|e| format!("query_path_info failed: {e:?}"))?
        .is_some());
    assert!(r3.map_err(|e| format!("Second is_valid_path failed: {e:?}"))?);

    let initial_request_count = *request_count_ref.lock().unwrap();
    assert_eq!(initial_request_count, 3, "Should have made 3 requests");

    // Stop the first server
    println!("Stopping first server...");
    server1.shutdown().await; // Shutdown all connections
    server_handle1.abort();
    let _ = server_handle1.await; // Wait for it to actually stop

    // Remove the socket file to ensure clean restart
    if socket_path.exists() {
        println!("Removing socket file: {socket_path:?}");
        std::fs::remove_file(&socket_path)?;
    }

    // Start a new server with a new handler immediately
    // The client's retry mechanism should handle any connection failures
    println!("Starting new server...");
    let handler2 = TestHandler::new("server2");
    let request_count_ref2 = handler2.request_count.clone();
    let (server2, server_handle2) = start_server(handler2, &socket_path).await;

    // Make concurrent requests - the client should handle the reconnection automatically
    // All pooled connections should fail, triggering retry logic for each
    println!("Making concurrent requests to new server...");

    let client1 = client.clone();
    let client2 = client.clone();
    let client3 = client.clone();
    let path1 = test_path.clone();
    let path2 = test_path.clone();
    let path3 = test_path.clone();

    // All three should retry and succeed with the new server
    let (r1, r2, r3) = tokio::join!(
        async {
            println!("Request 1 starting...");
            let result = client1.is_valid_path(&path1).await;
            println!("Request 1 result: {result:?}");
            result
        },
        async {
            println!("Request 2 starting...");
            let result = client2.query_path_info(&path2).await;
            println!("Request 2 result: {result:?}");
            result
        },
        async {
            println!("Request 3 starting...");
            let result = client3.is_valid_path(&path3).await;
            println!("Request 3 result: {result:?}");
            result
        }
    );

    assert!(r1?);
    assert!(r2?.is_some());
    assert!(r3?);

    let new_request_count = *request_count_ref2.lock().unwrap();
    println!("New server request count: {new_request_count}");
    assert_eq!(
        new_request_count, 3,
        "New server should have received 3 requests"
    );

    // Cleanup
    server2.shutdown().await;
    server_handle2.abort();

    Ok(())
}
