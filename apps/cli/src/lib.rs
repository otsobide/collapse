//! Command-line interface for Collapse: compress a file or folder, or extract
//! an archive, on top of `collapse-core` — or, with `--server`, through a
//! remote collapse-api instance.

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use collapse_core::{compress, compress_dir, extract, Algorithm};
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
    Compressed { output: PathBuf },
    Extracted { output_dir: PathBuf, files: Vec<String> },
}

impl Outcome {
    /// Print a human-readable summary to stdout.
    pub fn report(&self) {
        match self {
            Outcome::Compressed { output } => {
                println!("Created {}", output.display());
            }
            Outcome::Extracted { output_dir, files } => {
                println!("Extracted {} file(s) into {}", files.len(), output_dir.display());
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

    #[error("invalid path: {}", .0.display())]
    InvalidPath(PathBuf),

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
            server,
        } => run_compress(path, format, level, output, force, server),
        Command::Extract { archive, output } => run_extract(archive, output),
    }
}

fn run_compress(
    source: PathBuf,
    format: Option<Format>,
    level: u32,
    output: Option<PathBuf>,
    force: bool,
    server: Option<String>,
) -> Result<Outcome, CliError> {
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
        if output.canonicalize().map(|o| o == source).unwrap_or(false) {
            return Err(CliError::OutputIsSource(output));
        }
        if !force {
            return Err(CliError::OutputExists(output));
        }
    }

    if !source.is_dir() && !source.is_file() {
        return Err(CliError::UnsupportedSource(source));
    }

    match server.as_deref() {
        // Remote handles both shapes: a file goes as-is, a directory travels
        // as a tar envelope the server unwraps.
        Some(server) => {
            let archive = collapse_remote::compress_path(server, &source, algorithm, level)?;
            std::fs::write(&output, archive)?;
        }
        None if source.is_dir() => compress_dir(&source, &output, algorithm, level)?,
        None => {
            let arcname = source
                .file_name()
                .ok_or_else(|| CliError::InvalidPath(source.clone()))?
                .to_string_lossy()
                .into_owned();
            compress(&source, &output, &arcname, algorithm, level)?;
        }
    }

    Ok(Outcome::Compressed { output })
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
