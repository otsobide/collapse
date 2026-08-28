//! Command-line interface for Collapse: compress a file or folder, or extract
//! an archive, on top of `collapse-core` — or, with `--server`, through a
//! remote collapse-server-backend instance.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use collapse_core::paths::{inside, same_file};
use collapse_core::{
    compress, compress_dir, extract, unwritable_names_with, Algorithm, CharacterFault, NameProblem,
    NameReport, NameRules, Verify,
};
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
        /// The names as written, which is what the engine returns.
        ///
        /// "As written" is no longer a distinction: extraction refuses a name
        /// it cannot write rather than adjusting it, so these are the names the
        /// archive spells. The wording stays because the guarantee is worth
        /// stating either way.
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
    /// `--format` and `-o`'s extension name different formats.
    ///
    /// Refused rather than resolved, because either resolution leaves the user
    /// with a file whose name does not describe it. Decidable from the
    /// arguments alone, so it is reported before anything is read or written.
    #[error(
        "--format says {asked} but {} ends in .{}, which is a different format. \
         Drop one of the two.",
        output.display(),
        implied.extension()
    )]
    FormatContradictsOutput {
        asked: Algorithm,
        implied: Algorithm,
        output: PathBuf,
    },

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

    /// The archive holds entry names this machine cannot write, and at least
    /// one of them needs an answer nobody can be asked for here.
    ///
    /// The whole message is built by [`unwritable_entries_message`]: it is the
    /// only thing a user gets, so it is worth more than a sentence.
    #[error("{}", unwritable_entries_message(.archive, .report))]
    UnwritableEntries {
        archive: PathBuf,
        report: NameReport,
    },

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

    // Both spellings are kept from here on, and the distinction matters at
    // every use.
    //
    // The **resolved** one is what the guards and the engine need: `.`, `..`
    // and a trailing slash have to become a real path with a usable file name,
    // and an output that aliases the source can only be spotted against a
    // resolved path.
    //
    // The **typed** one is what the user gets told. Answering `./sub/a.txt`
    // with `/Users/…/sub/a.txt.zip` makes a person check whether the tool
    // understood them (issue #67).
    let typed = source;
    let resolved = typed
        .canonicalize()
        .map_err(|_| CliError::NotFound(typed.clone()))?;

    // Explicit --format wins; otherwise infer from the output's extension, else zip.
    let algorithm = resolve_format(format, output.as_deref())?;

    let output = match output {
        Some(path) => path,
        None => default_output_path(&typed, &resolved, algorithm)?,
    };

    if output.exists() {
        // Both guards ask the filesystem whether these are the same file, not
        // whether they are spelled the same: a hardlink is a second name for
        // one file, so it never resolves to the same path. Comparing paths
        // here is what let --force overwrite its own source.
        if same_file(&resolved, &output) {
            return Err(CliError::OutputIsSource(output));
        }
        // Inside the tree being archived, and --force cannot buy past it. The
        // backends list the tree before creating the archive, so this file
        // would be truncated and then archived in its truncated state: lost
        // from the archive as much as from disk, and the archive corrupt with
        // it. Same reasoning as OutputIsSource above, which is also ahead of
        // the force check.
        if resolved.is_dir() && inside(&resolved, &output) {
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

    if !resolved.is_dir() && !resolved.is_file() {
        return Err(CliError::UnsupportedSource(typed));
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
            let archive = collapse_remote::compress_path(server, &resolved, algorithm, level)?;
            std::fs::write(&output, archive)?;
            None
        }
        None if resolved.is_dir() => {
            compress_dir(&resolved, &output, algorithm, level, depth)?;
            Some(depth)
        }
        None => {
            // The resolved spelling, deliberately: this is the name stored
            // inside the archive, so it has to be the file's real one. `.` and
            // a trailing slash have no name to give.
            let arcname = resolved
                .file_name()
                .ok_or_else(|| CliError::InvalidPath(typed.clone()))?
                .to_string_lossy()
                .into_owned();
            compress(&resolved, &output, &arcname, algorithm, level, depth)?;
            Some(depth)
        }
    };

    Ok(Outcome::Compressed { output, checked })
}

/// Resolve the archive format: explicit `--format`, else the output file's
/// extension if it names a known format, else zip.
///
/// **A `--format` that contradicts `-o`'s extension is refused**, rather than
/// letting one win. It used to let `--format` win in silence, which produced a
/// file whose name lies about its contents and that this same CLI then rejects:
///
/// ```text
/// $ collapse compress notes.txt -f tar -o mixed.zip
/// Created mixed.zip
/// $ collapse extract mixed.zip -o out
/// error: Could not find EOCD
/// ```
///
/// A warning was the other candidate and is worse: it goes to a terminal nobody
/// is reading in a script, and what is left behind is still a broken file with
/// a misleading name. This is decidable from the arguments alone, before any
/// work, so it is the cheapest possible moment to say so (issue #75).
///
/// An extension that names no known format is **not** a contradiction: `-o
/// backup.bin -f 7z` is a deliberate choice, and only an extension that names a
/// *different* format is a mistake.
fn resolve_format(format: Option<Format>, output: Option<&Path>) -> Result<Algorithm, CliError> {
    let from_output = output
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .and_then(Algorithm::from_extension);

    match (format, from_output) {
        (Some(asked), Some(implied)) => {
            let asked: Algorithm = asked.into();
            if asked != implied {
                return Err(CliError::FormatContradictsOutput {
                    asked,
                    implied,
                    output: output.unwrap_or(Path::new("")).to_path_buf(),
                });
            }
            Ok(asked)
        }
        (Some(asked), None) => Ok(asked.into()),
        (None, Some(implied)) => Ok(implied),
        (None, None) => Ok(Algorithm::Zip),
    }
}

fn run_extract(archive: PathBuf, output_dir: PathBuf) -> Result<Outcome, CliError> {
    if !archive.exists() {
        return Err(CliError::NotFound(archive));
    }

    // What this machine cannot name, read from the archive's listing before
    // anything is created. The rules are the host's, because the question is
    // what this filesystem can hold and not what would travel elsewhere: on
    // Unix almost every name is fine, and on Windows an ordinary tarball built
    // on Linux can be full of names it will not take. Bound once and used for
    // both the report and the adjustments, so the two cannot disagree about
    // which machine they are talking about.
    let rules = NameRules::host();
    // A listing this build cannot read is deliberately not this check's
    // business. Extraction is about to open the same archive and fail on it in
    // its own vocabulary ("Unknown archive extension", "invalid Zip archive"),
    // which is the message this pass would otherwise replace with a worse one
    // about names. Core skips its own planning pass for the same reason.
    let report = unwritable_names_with(&archive, rules).unwrap_or_default();

    // Every problem, not only the ones that used to carry a question. There is
    // no question left to ask: core refuses an entry it cannot write under the
    // archive's own name rather than adjusting it, so a trailing dot and a
    // reserved device stop the extraction exactly as a colon does. Refusing
    // here as well is not redundant — it is what lets the message name *every*
    // entry at fault from the one listing, where core stops at the first.
    if !report.is_empty() {
        return Err(CliError::UnwritableEntries { archive, report });
    }
    let files = extract(&archive, &output_dir)?;
    Ok(Outcome::Extracted { output_dir, files })
}

/// Why an archive was refused, which entries are at fault, what is wrong with
/// each, and what would let the user get their files.
///
/// Public and pure so it can be read back on any machine: a [`NameReport`] is
/// data, and `NameReport::of(names, NameRules::windows())` builds the Windows
/// one from a Mac. This message is the entire feature on the command line, and
/// a message only Windows can produce is a message nobody here would ever read
/// before a user does.
pub fn unwritable_entries_message(archive: &Path, report: &NameReport) -> String {
    let mut message = String::new();
    let count = report.entries.len();
    let plural = if count == 1 { "name" } else { "names" };
    // `write!` into a String cannot fail, so the results are dropped rather
    // than dressed up as an error this function has no way to return.
    let _ = write!(
        message,
        "cannot extract {}: {count} entry {plural} cannot be written on this system",
        archive.display()
    );

    for unwritable in &report.entries {
        // Quoted, because a name whose fault is a trailing space says nothing
        // at all unquoted.
        let _ = write!(message, "\n  {:?}", unwritable.entry);
        for problem in &unwritable.problems {
            let _ = write!(message, "\n    {}", explain(problem));
        }
    }

    let _ = write!(
        message,
        "\nNothing was extracted, and nothing this command could be told would change that: \
         extraction writes every entry under the name the archive spells or it writes none of \
         them. Extract on a system that can hold these names."
    );
    if !report.characters.is_empty() {
        // Named even though there is no answer to give, because it is what
        // tells a user whether the archive is unusable here or merely awkward:
        // one character across forty entries is a different problem from forty
        // characters.
        let _ = write!(message, " The characters at fault are {}.", listed(report));
    }
    message
}

/// One line for one problem, about the machine in front of the user ("here")
/// rather than about filesystems in general, because that is what the rules
/// are: what this host can hold, not what is portable.
fn explain(problem: &NameProblem) -> String {
    match problem {
        NameProblem::Character {
            character,
            fault: CharacterFault::Rejected,
        } => format!("{character:?} cannot appear in a file name here"),
        // The loud/silent split is the whole difference between issues #64 and
        // #63, and it is what the user needs to hear: one is a name the system
        // refuses, the other is a name it accepts and reads as something else.
        NameProblem::Character {
            character,
            fault: CharacterFault::Reinterpreted,
        } => format!(
            "{character:?} is not read as part of a name here: the entry would be attached to \
             another file as hidden data instead of becoming a file, with no error"
        ),
        NameProblem::TrailingCharacters { removed } => format!(
            "the name ends in {removed:?}, which this system does not keep, so the file would not \
             be the one the archive names"
        ),
        NameProblem::ReservedDevice { device } => {
            format!("{device:?} names a device rather than a file, in every directory")
        }
    }
}

/// The characters holding the archive up, with how much of it each holds up:
/// `'?' (2 entries) and ':' (1 entry)`.
fn listed(report: &NameReport) -> String {
    let parts: Vec<String> = report
        .characters
        .iter()
        .map(|offender| {
            let unit = if offender.entries == 1 {
                "entry"
            } else {
                "entries"
            };
            format!("{:?} ({} {unit})", offender.character, offender.entries)
        })
        .collect();
    match parts.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// Derive the default archive path: `<source>.<ext>` next to the source.
fn default_output_path(
    typed: &Path,
    resolved: &Path,
    algorithm: Algorithm,
) -> Result<PathBuf, CliError> {
    // The name comes from the resolved path, which is the one guaranteed to
    // have one: `.` and a trailing slash do not.
    let name = resolved
        .file_name()
        .ok_or_else(|| CliError::InvalidPath(resolved.to_path_buf()))?;
    let mut file_name = name.to_os_string();
    file_name.push(".");
    file_name.push(algorithm.extension());

    match typed.file_name() {
        // The user spelled a name, so the archive goes beside it in their
        // spelling: `./sub/a.txt` yields `./sub/a.txt.zip`, not an absolute
        // path they never typed.
        Some(_) => Ok(typed.with_file_name(file_name)),
        // `.`, `..` or a bare root. There is no spelling to preserve, and the
        // archive belongs beside the directory rather than inside it, which
        // only the resolved path can express.
        None => Ok(resolved.with_file_name(file_name)),
    }
}
