pub mod compression;
// Path predicates the front ends share. Not part of compressing anything, but
// they belong to whatever both front ends depend on: keeping a copy in each is
// what let them drift apart, and the drift cost the CLI its data-loss guard.
pub mod paths;

pub use compression::{compress, compress_dir, extract, Algorithm, CompressionError};
