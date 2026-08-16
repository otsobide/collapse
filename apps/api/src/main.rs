use std::net::SocketAddr;

use clap::Parser;

use collapse_api::{build_router, DEFAULT_MAX_UPLOAD_MB};

/// Collapse compression API server.
#[derive(Parser)]
#[command(name = "collapse-api", version, about)]
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
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let addr: SocketAddr = format!("{}:{}", cli.host, cli.port)
        .parse()
        .expect("Invalid address");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("Failed to bind");

    println!("collapse-api listening on {addr}");

    axum::serve(listener, build_router(cli.max_upload_mb))
        .await
        .expect("Server error");
}
