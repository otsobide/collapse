use thiserror::Error;

/// What can go wrong talking to a remote Collapse server.
///
/// The variants separate the cases a caller may want to act on differently:
/// the server was not reachable at all, it answered with an error status, the
/// compression itself failed on the far side, or it is not speaking this
/// protocol.
#[derive(Debug, Error)]
pub enum RemoteError {
    /// No HTTP exchange happened: DNS, connection refused, timeout, TLS.
    #[error("cannot reach the server at {server}: {reason}")]
    Unreachable { server: String, reason: String },

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

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
