use std::time::Duration;

use thiserror::Error;

/// What can go wrong talking to a remote Collapse server.
///
/// The variants separate the cases a caller may want to act on differently:
/// the server was not reachable at all, it answered with an error status, the
/// compression itself failed on the far side, or it is not speaking this
/// protocol.
#[derive(Debug, Error)]
pub enum RemoteError {
    /// The address is not an address at all: empty, or nothing but
    /// whitespace. Refused before any request is attempted, because a
    /// transport failure naming a server with no name reads as a network
    /// problem when the real mistake is the destination.
    #[error("the server address is blank: it needs a URL, for example http://localhost:8000")]
    BlankServer,

    /// No HTTP exchange happened: DNS, connection refused, timeout, TLS.
    #[error("cannot reach the server at {server}: {reason}")]
    Unreachable { server: String, reason: String },

    /// A socket was open and the far side stopped answering on it.
    ///
    /// Kept apart from [`Self::Unreachable`] because the two call for different
    /// things from whoever reads it: an address that cannot be reached is
    /// usually wrong or the server is down, while a server that accepted the
    /// connection and then went silent is running and stuck. The job may well
    /// still exist on the far side.
    ///
    /// The limit this reports is on the **server's answers**, never on the
    /// compression: a job is free to run for hours as long as each poll is
    /// answered (see `Timeouts`).
    #[error(
        "the server at {server} accepted the connection and then stopped answering \
         (nothing for {} seconds). The job may still be running there; the archive was not \
         downloaded. Underlying error: {reason}",
        after.as_secs()
    )]
    Unresponsive {
        server: String,
        after: Duration,
        reason: String,
    },

    /// The server answered with a 4xx/5xx. `message` is already rendered for
    /// a human (it prefers the server's JSON `detail` field).
    #[error("{message}")]
    Rejected { status: u16, message: String },

    /// The job reached the `failed` state; this is the server's own message.
    #[error("server-side error: {0}")]
    Failed(String),

    /// The server answered something this protocol does not understand.
    #[error("{0}")]
    Malformed(String),

    /// The source could not be prepared for upload: no usable name, or the
    /// directory could not be packed into the tar envelope.
    #[error("cannot prepare {path} for upload: {reason}")]
    Packing { path: String, reason: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
