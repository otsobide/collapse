use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

use collapse_server_backend::{
    build_app_with, logging, DEFAULT_JOB_TTL_MINUTES, DEFAULT_MAX_UPLOAD_MB,
};

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

    /// Delete finished jobs nobody downloads again after this many minutes.
    /// 0 keeps them until a client deletes them.
    #[arg(long, default_value_t = DEFAULT_JOB_TTL_MINUTES)]
    job_ttl_minutes: u64,

    /// On a stop, how long to keep serving the requests already in flight
    /// before exiting anyway.
    #[arg(long, default_value_t = DEFAULT_SHUTDOWN_GRACE_SECONDS)]
    shutdown_grace_seconds: u64,
}

/// How long a stop waits for what is already in flight.
///
/// Docker's own grace period is ten seconds by default, after which it sends
/// SIGKILL and none of this matters, so a deployment that wants long transfers
/// to survive a restart has to raise `stop_grace_period` to match.
const DEFAULT_SHUTDOWN_GRACE_SECONDS: u64 = 10;

/// Wait for a stop, then let the server drain.
///
/// Returning from this future is what makes axum stop accepting connections
/// while it finishes the requests it already has. The watchdog is the other
/// half: a client that stops reading its download would otherwise hold the
/// process open for as long as it liked.
async fn shutdown_signal(grace: Duration) {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            // Nothing to wait for, so wait forever and let ctrl_c decide.
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = interrupt => {}
        _ = terminate => {}
    }

    tracing::info!(
        grace_seconds = grace.as_secs(),
        "stopping: no new connections, finishing what is in flight"
    );

    tokio::spawn(async move {
        tokio::time::sleep(grace).await;
        tracing::warn!("requests were still in flight after the grace period, exiting anyway");
        std::process::exit(0);
    });
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

    // Built before announcing the port: it opens the registry and reconciles
    // it against the staging directory, and a server that cannot do that has
    // nothing to serve.
    // Zero disables the reaper; anything else is the window a finished job has
    // to be downloaded again before it is collected.
    let job_ttl = (cli.job_ttl_minutes > 0).then(|| Duration::from_secs(cli.job_ttl_minutes * 60));

    let app = build_app_with(storage_dir.clone(), cli.max_upload_mb, job_ttl)
        .unwrap_or_else(|e| fatal(e.to_string()));

    // The address the socket actually got, not the one that was asked for:
    // with `--port 0` the operating system picks, and the log is the only
    // place that says which.
    let bound = listener.local_addr().unwrap_or(addr);

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        addr = %bound,
        max_upload_mb = cli.max_upload_mb,
        job_ttl_minutes = cli.job_ttl_minutes,
        storage_dir = %storage_dir.display(),
        "collapse-server-backend listening"
    );

    let grace = Duration::from_secs(cli.shutdown_grace_seconds);
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(grace))
        .await
    {
        fatal(format!("Server error: {e}"));
    }

    // Reached only on a clean stop, which is also what lets the staging
    // TempDir's guard run: a process killed mid-response leaves it behind.
    tracing::info!("stopped");
}
