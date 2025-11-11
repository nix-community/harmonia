pub mod connection;
pub mod handler;

pub use handler::RequestHandler;

use crate::error::{IoErrorContext, ProtocolError};
use harmonia_store_core::store_path::StoreDir;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

pub struct DaemonServer<H: RequestHandler> {
    handler: H,
    socket_path: PathBuf,
    store_dir: StoreDir,
    connection_handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl<H: RequestHandler + Clone + 'static> DaemonServer<H> {
    pub fn new(handler: H, socket_path: PathBuf) -> Self {
        Self {
            handler,
            socket_path,
            store_dir: StoreDir::default(),
            connection_handles: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn serve(&self) -> Result<(), ProtocolError> {
        // Create socket in a temporary location first
        let socket_dir = self
            .socket_path
            .parent()
            .unwrap_or(std::path::Path::new("."));
        let temp_socket = socket_dir.join(format!(
            ".{}.tmp",
            self.socket_path.file_name().unwrap().to_string_lossy()
        ));

        // Remove any existing temporary socket
        let _ = std::fs::remove_file(&temp_socket);

        // Bind to the temporary socket
        let listener = UnixListener::bind(&temp_socket).io_context(format!(
            "Failed to bind to temporary socket path: {temp_socket:?}"
        ))?;

        // Set correct permissions on the temporary socket
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp_socket, std::fs::Permissions::from_mode(0o666))
            .io_context(format!("Failed to set socket permissions: {temp_socket:?}"))?;

        // Atomically move the socket to the final location
        std::fs::rename(&temp_socket, &self.socket_path).io_context(format!(
            "Failed to move socket from {:?} to {:?}",
            temp_socket, self.socket_path
        ))?;

        loop {
            let (stream, _) = listener
                .accept()
                .await
                .io_context("Failed to accept connection")?;
            let handler = self.handler.clone();
            let store_dir = self.store_dir.clone();

            let handle = tokio::spawn(async move {
                if let Err(e) = connection::handle_connection(stream, handler, store_dir).await {
                    eprintln!("Connection error: {e}");
                }
            });

            // Store the handle
            self.connection_handles.lock().await.push(handle);

            // Clean up finished handles
            let mut handles = self.connection_handles.lock().await;
            handles.retain(|h| !h.is_finished());
        }
    }

    /// Shutdown all active connections
    pub async fn shutdown(&self) {
        let handles = self.connection_handles.lock().await;
        for handle in handles.iter() {
            handle.abort();
        }
    }
}
