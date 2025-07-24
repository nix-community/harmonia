pub mod connection;
pub mod handler;

pub use handler::RequestHandler;

use crate::error::{IoErrorContext, ProtocolError};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

pub struct DaemonServer<H: RequestHandler> {
    handler: H,
    socket_path: PathBuf,
    connection_handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl<H: RequestHandler + Clone + 'static> DaemonServer<H> {
    pub fn new(handler: H, socket_path: PathBuf) -> Self {
        Self {
            handler,
            socket_path,
            connection_handles: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn serve(&self) -> Result<(), ProtocolError> {
        let listener = UnixListener::bind(&self.socket_path).io_context(format!(
            "Failed to bind to socket path: {:?}",
            self.socket_path
        ))?;

        loop {
            let (stream, _) = listener
                .accept()
                .await
                .io_context("Failed to accept connection")?;
            let handler = self.handler.clone();

            let handle = tokio::spawn(async move {
                if let Err(e) = connection::handle_connection(stream, handler).await {
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
