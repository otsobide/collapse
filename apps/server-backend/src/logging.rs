//! Log setup.
//!
//! One line per event on stdout, each carrying an RFC 3339 timestamp, a level
//! and the target that emitted it, which is the shape `docker logs`, journald
//! and every log shipper already know how to read. The level is chosen with
//! `RUST_LOG`, the same variable every other tracing-based service uses.

use std::io::IsTerminal;

use tracing_subscriber::EnvFilter;

/// Filter applied when `RUST_LOG` says nothing: our own events and one line
/// per request, without the per-request DEBUG chatter of the middleware.
pub const DEFAULT_FILTER: &str = "info";

/// Build the log filter from a `RUST_LOG`-style spec.
///
/// An empty or unparseable spec falls back to [`DEFAULT_FILTER`]. A typo in an
/// environment variable is no reason for a server to refuse to start, and
/// starting silently mute would hide more than it saves.
pub fn filter(spec: Option<&str>) -> EnvFilter {
    spec.map(str::trim)
        .filter(|spec| !spec.is_empty())
        .and_then(|spec| EnvFilter::try_new(spec).ok())
        .unwrap_or_else(|| EnvFilter::new(DEFAULT_FILTER))
}

/// Install the subscriber for this process. Call it once, before anything
/// worth logging happens.
pub fn init() {
    let spec = std::env::var("RUST_LOG").ok();
    // Checked before the subscriber exists, reported once it does.
    let ignored = spec
        .as_deref()
        .map(str::trim)
        .is_some_and(|spec| !spec.is_empty() && EnvFilter::try_new(spec).is_err());

    tracing_subscriber::fmt()
        .with_env_filter(filter(spec.as_deref()))
        // Colour codes belong in a terminal, not in a log file or in the
        // output `docker logs` hands to a shipper.
        .with_ansi(std::io::stdout().is_terminal())
        .init();

    if ignored {
        tracing::warn!("RUST_LOG could not be parsed, using {DEFAULT_FILTER} instead");
    }
}
