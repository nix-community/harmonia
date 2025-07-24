pub mod connection;
pub mod handler;

pub use handler::RequestHandler;

use crate::error::ProtocolError;
use std::path::PathBuf;
use tokio::net::UnixListener;

pub struct DaemonServer<H: RequestHandler> {
    handler: H,
    socket_path: PathBuf,
}

impl<H: RequestHandler + Clone + 'static> DaemonServer<H> {
    pub fn new(handler: H, socket_path: PathBuf) -> Self {
        Self {
            handler,
            socket_path,
        }
    }

    pub async fn serve(&self) -> Result<(), ProtocolError> {
        let listener = UnixListener::bind(&self.socket_path)?;

        loop {
            let (stream, _) = listener.accept().await?;
            let handler = self.handler.clone();

            tokio::spawn(async move {
                if let Err(e) = connection::handle_connection(stream, handler).await {
                    eprintln!("Connection error: {e}");
                }
            });
        }
    }
}
