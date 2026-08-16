use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

use collapse_server_backend::{build_app, DEFAULT_MAX_UPLOAD_MB};

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

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Keep the TempDir guard alive for the whole run so the default staging
    // area is cleaned up when the server exits.
    let (storage_dir, _storage_guard) = match cli.storage_dir {
        Some(dir) => (dir, None),
        None => {
            let tmp = tempfile::tempdir().expect("Failed to create the staging directory");
            (tmp.path().to_path_buf(), Some(tmp))
        }
    };

    let addr: SocketAddr = format!("{}:{}", cli.host, cli.port)
        .parse()
        .expect("Invalid address");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind");

    println!("collapse-server-backend listening on {addr}");

    axum::serve(listener, build_app(storage_dir, cli.max_upload_mb))
        .await
        .expect("Server error");
}
