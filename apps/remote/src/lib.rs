//! Client for a remote Collapse compression server (`collapse-api`).
//!
//! The server compresses asynchronously: uploading answers `202 Accepted` with
//! a job, the job is polled until it settles, the archive is downloaded and the
//! job is deleted. [`compress_path`] performs that whole exchange and hands
//! back the archive bytes, for a single file or for a whole directory (packed
//! into a tar envelope the server unwraps).
//!
//! The crate is split so the decisions can be tested without a server:
//! [`protocol`] holds the pure ones (URL building, reading the server's JSON,
//! what to do with each job status) and the HTTP plumbing stays private.
//!
//! It exists as its own crate because more than one front-end needs it: the
//! CLI's `--server` flag today, the desktop app next. Duplicating the exchange
//! per front-end is exactly the kind of drift this project avoids.

mod client;
mod error;

pub mod protocol;

pub use client::compress_path;
pub use error::RemoteError;
