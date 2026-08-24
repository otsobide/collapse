//! Command-line interface for Collapse: compress a file or folder, or extract
//! an archive, on top of `collapse-core` — or, with `--server`, through a
//! remote collapse-server-backend instance.

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use collapse_core::paths::{inside, same_file};
use collapse_core::{compress, compress_dir, extract, Algorithm, Verify};
use thiserror::Error;

/// Compress and extract files and folders.
#[derive(Debug, Parser)]
#[command(name = "collapse", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Compress a file or folder into an archive.
    #[command(alias = "c")]
    Compress {
        /// File or directory to compress.
        path: PathBuf,

        /// Archive format [default: zip, or inferred from --output's extension].
        #[arg(short, long, value_enum)]
        format: Option<Format>,

        /// Compression level, 1 (fastest) to 5 (smallest). Ignored by tar.
        #[arg(short, long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=5))]
        level: u32,

        /// Output archive path (default: alongside the source, named <source>.<ext>).
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Overwrite the output archive if it already exists.
        #[arg(long)]
        force: bool,

        /// Read every entry back, not just the archive's listing: about twice
        /// the work, and it checks the per-entry checksums zip and 7z store
        /// (tar stores none).
        #[arg(long)]
        verify: bool,

        /// Compress on a remote Collapse server instead of locally
        /// (e.g. http://localhost:8000).
        #[arg(long, value_name = "URL")]
        server: Option<String>,
    },

    /// Extract an archive (.zip, .7z or .tar) — format detected by extension.
    #[command(alias = "e")]
    Extract {
        /// Archive to extract.
        archive: PathBuf,

        /// Directory to extract into.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
    },
}

/// Archive formats selectable on the command line.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Format {
    #[value(name = "7z")]
    SevenZ,
    Zip,
    Tar,
}

impl From<Format> for Algorithm {
    fn from(format: Format) -> Self {
        match format {
            Format::SevenZ => Algorithm::SevenZ,
            Format::Zip => Algorithm::Zip,
            Format::Tar => Algorithm::Tar,
        }
    }
}

/// What a command did, so the caller can report it.
#[derive(Debug)]
pub enum Outcome {
    Compressed {
        output: PathBuf,
        /// How thoroughly the archive was checked before it landed at
        /// `output`, or `None` when this side checked nothing at all.
        ///
        /// `None` is the remote path: the archive arrives already finished and
        /// the list of entries to hold it against is the server's, not ours.
        /// Reporting a depth there would claim a check that never ran, and the
        /// whole point of the check is that a user can trust it.
        checked: Option<Verify>,
    },
    Extracted {
        output_dir: PathBuf,
        files: Vec<String>,
    },
}

impl Outcome {
    /// Print a human-readable summary to stdout.
    pub fn report(&self) {
        match self {
            // Only the deeper check is mentioned, because it is the only one
            // the user asked for and paid for. The listing check runs on every
            // local compression, so announcing it would be noise on every
            // single run.
            Outcome::Compressed {
                output,
                checked: Some(Verify::Contents),
            } => {
                println!("Created {} (contents verified)", output.display());
            }
            Outcome::Compressed { output, .. } => {
                println!("Created {}", output.display());
            }
            Outcome::Extracted { output_dir, files } => {
                println!(
                    "Extracted {} file(s) into {}",
                    files.len(),
                    output_dir.display()
                );
                for file in files {
                    println!("  {file}");
                }
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error("path not found: {}", .0.display())]
    NotFound(PathBuf),

    #[error("unsupported source (not a regular file or directory): {}", .0.display())]
    UnsupportedSource(PathBuf),

    #[error("output already exists: {} (use --force to overwrite)", .0.display())]
    OutputExists(PathBuf),

    #[error("refusing to write the archive onto its own source: {}", .0.display())]
    OutputIsSource(PathBuf),

    #[error("refusing to write the archive inside the folder being compressed: {} (it would be destroyed instead of archived)", .0.display())]
    OutputInsideSource(PathBuf),

    #[error("invalid path: {}", .0.display())]
    InvalidPath(PathBuf),

    /// `--verify` asks for work that happens where the archive is built, and
    /// with `--server` that is the other end of the wire.
    ///
    /// Refused rather than ignored: a flag that asks for a stronger guarantee
    /// is the last one to silently do nothing. It is also not something this
    /// side can make up for by checking the download, because for a directory
    /// it has no list of entries to hold the archive against.
    #[error("--verify cannot be used with --server: the archive is built on the server, which this build has no way to ask for that check (compress locally to use --verify)")]
    RemoteVerifyUnsupported,

    #[error(transparent)]
    Remote(#[from] collapse_remote::RemoteError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Core(#[from] collapse_core::CompressionError),
}

/// Run a parsed CLI invocation.
pub fn run(cli: Cli) -> Result<Outcome, CliError> {
    match cli.command {
        Command::Compress {
            path,
            format,
            level,
            output,
            force,
            verify,
            server,
        } => run_compress(path, format, level, output, force, verify, server),
        Command::Extract { archive, output } => run_extract(archive, output),
    }
}

fn run_compress(
    source: PathBuf,
    format: Option<Format>,
    level: u32,
    output: Option<PathBuf>,
    force: bool,
    verify: bool,
    server: Option<String>,
) -> Result<Outcome, CliError> {
    // First, ahead of the filesystem: this one is a mistake in the command
    // itself, decidable from the arguments alone. Reporting "output already
    // exists" first would send the user off to add --force and meet this on
    // the next run.
    if verify && server.is_some() {
        return Err(CliError::RemoteVerifyUnsupported);
    }

    // Canonicalize so `.`/`..`/trailing slashes resolve to a real path with a
    // usable file name (and to detect an output that aliases the source).
    let source = source
        .canonicalize()
        .map_err(|_| CliError::NotFound(source))?;

    // Explicit --format wins; otherwise infer from the output's extension, else zip.
    let algorithm = resolve_format(format, output.as_deref());

    let output = match output {
        Some(path) => path,
        None => default_output_path(&source, algorithm)?,
    };

    if output.exists() {
        // Both guards ask the filesystem whether these are the same file, not
        // whether they are spelled the same: a hardlink is a second name for
        // one file, so it never resolves to the same path. Comparing paths
        // here is what let --force overwrite its own source.
        if same_file(&source, &output) {
            return Err(CliError::OutputIsSource(output));
        }
        // Inside the tree being archived, and --force cannot buy past it. The
        // backends list the tree before creating the archive, so this file
        // would be truncated and then archived in its truncated state: lost
        // from the archive as much as from disk, and the archive corrupt with
        // it. Same reasoning as OutputIsSource above, which is also ahead of
        // the force check.
        if source.is_dir() && inside(&source, &output) {
            return Err(CliError::OutputInsideSource(output));
        }
        if !force {
            return Err(CliError::OutputExists(output));
        }
        // Deliberately NOT unlinked here. The write happens only once the
        // archive is fully in hand (the remote path downloads it all before
        // touching disk), so a failed run leaves the previous archive exactly
        // as it was. Removing it up front would trade that away for nothing.
    }

    if !source.is_dir() && !source.is_file() {
        return Err(CliError::UnsupportedSource(source));
    }

    // Every local compression is checked; --verify only says how deeply. The
    // depth is bound once and then both handed to the engine and reported, so
    // the Outcome cannot end up naming a check that did not happen.
    let depth = if verify {
        Verify::Contents
    } else {
        Verify::Index
    };

    let checked = match server.as_deref() {
        // Remote handles both shapes: a file goes as-is, a directory travels
        // as a tar envelope the server unwraps.
        Some(server) => {
            let archive = collapse_remote::compress_path(server, &source, algorithm, level)?;
            std::fs::write(&output, archive)?;
            None
        }
        None if source.is_dir() => {
            compress_dir(&source, &output, algorithm, level, depth)?;
            Some(depth)
        }
        None => {
            let arcname = source
                .file_name()
                .ok_or_else(|| CliError::InvalidPath(source.clone()))?
                .to_string_lossy()
                .into_owned();
            compress(&source, &output, &arcname, algorithm, level, depth)?;
            Some(depth)
        }
    };

    Ok(Outcome::Compressed { output, checked })
}

/// Resolve the archive format: explicit `--format`, else the output file's
/// extension if it names a known format, else zip.
fn resolve_format(format: Option<Format>, output: Option<&Path>) -> Algorithm {
    if let Some(format) = format {
        return format.into();
    }
    output
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .and_then(Algorithm::from_extension)
        .unwrap_or(Algorithm::Zip)
}

fn run_extract(archive: PathBuf, output_dir: PathBuf) -> Result<Outcome, CliError> {
    if !archive.exists() {
        return Err(CliError::NotFound(archive));
    }
    let files = extract(&archive, &output_dir)?;
    Ok(Outcome::Extracted { output_dir, files })
}

/// Derive the default archive path: `<source>.<ext>` next to the source.
fn default_output_path(source: &Path, algorithm: Algorithm) -> Result<PathBuf, CliError> {
    let name = source
        .file_name()
        .ok_or_else(|| CliError::InvalidPath(source.to_path_buf()))?;
    let mut file_name = name.to_os_string();
    file_name.push(".");
    file_name.push(algorithm.extension());
    Ok(source.with_file_name(file_name))
}
