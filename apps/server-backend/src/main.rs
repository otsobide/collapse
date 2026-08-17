use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

use collapse_server_backend::{build_app, logging, DEFAULT_MAX_UPLOAD_MB};

/// Collapse compression API server.
#[derive(Parser)]
#[command(name = "collapse-server-backend", version, about)]
struct Cli {
    /// Host address to bind to.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// Port to listen on.
    #[arg(long, default_value_t = 8000)]
    port: u16,

    /// Maximum accepted upload size, in mebibytes.
    #[arg(long, default_value_t = DEFAULT_MAX_UPLOAD_MB)]
    max_upload_mb: usize,

    /// Directory to stage job files in (default: a temporary directory
    /// removed when the server stops).
    #[arg(long)]
    storage_dir: Option<PathBuf>,
}

/// Report a startup failure the way the rest of the server reports events, and
/// exit. A panic would carry the same information in a shape nothing parses.
fn fatal(message: String) -> ! {
    tracing::error!("{message}");
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    logging::init();

    // Keep the TempDir guard alive for the whole run so the default staging
    // area is cleaned up when the server exits.
    let (storage_dir, _storage_guard) = match cli.storage_dir {
        Some(dir) => (dir, None),
        None => {
            let tmp = tempfile::tempdir()
                .unwrap_or_else(|e| fatal(format!("Cannot create the staging directory: {e}")));
            (tmp.path().to_path_buf(), Some(tmp))
        }
    };

    let addr: SocketAddr = format!("{}:{}", cli.host, cli.port)
        .parse()
        .unwrap_or_else(|_| fatal(format!("Invalid address: {}:{}", cli.host, cli.port)));

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| fatal(format!("Cannot bind to {addr}: {e}")));

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        %addr,
        max_upload_mb = cli.max_upload_mb,
        storage_dir = %storage_dir.display(),
        "collapse-server-backend listening"
    );

    if let Err(e) = axum::serve(listener, build_app(storage_dir, cli.max_upload_mb)).await {
        fatal(format!("Server error: {e}"));
    }
}
