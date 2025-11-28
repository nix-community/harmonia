use harmonia_daemon::config::Config;
use harmonia_daemon::error::{DaemonError, IoContext};
use harmonia_daemon::handler::LocalStoreHandler;
use harmonia_daemon::server::DaemonServer;
use log::{error, info};
use std::path::PathBuf;
use tokio::signal;

#[tokio::main]
async fn main() -> Result<(), DaemonError> {
    // Initialize logger
    env_logger::init();

    // Load configuration
    let config = match std::env::var("HARMONIA_DAEMON_CONFIG") {
        Ok(path) => Config::from_file(&PathBuf::from(path))?,
        Err(_) => Config::default(),
    };

    info!("Starting harmonia-daemon");
    info!("Socket path: {}", config.socket_path.display());
    info!("Store directory: {}", config.store_dir.display());
    info!("Database path: {}", config.db_path.display());

    // Create the local store handler
    let handler = LocalStoreHandler::new(config.store_dir.clone(), config.db_path).await?;

    // Create StoreDir for protocol serialization
    let store_dir = harmonia_store_core::store_path::StoreDir::new(&config.store_dir)
        .map_err(|e| DaemonError::config(format!("Invalid store directory: {e}")))?;

    // Create and start the daemon server
    let server = DaemonServer::new(handler, config.socket_path.clone(), store_dir);

    // Set up signal handlers
    let shutdown = shutdown_signal();

    // Run the server
    tokio::select! {
        result = server.serve() => {
            if let Err(e) = result {
                error!("Server error: {e}");
                return Err(DaemonError::io("Server error", e));
            }
        }
        _ = shutdown => {
            info!("Received shutdown signal");
        }
    }

    // Clean up: remove socket file
    if config.socket_path.exists() {
        std::fs::remove_file(&config.socket_path).io_context(|| {
            format!(
                "Failed to remove socket file at {}",
                config.socket_path.display()
            )
        })?;
    }

    info!("harmonia-daemon stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
