//! The path guards the commands rely on.
//!
//! Both predicates now live in `collapse-core`, because the CLI needs exactly
//! the same two and keeping a copy in each front end is what let them drift:
//! this one compared filesystem identity but only on Unix, and the CLI's
//! compared resolved paths and nothing else. They are re-exported here so the
//! commands and `tests/paths.rs` keep one import, and so this module stays the
//! place to look for "what stops the app writing over your data".

pub use collapse_core::paths::{inside, same_file};
