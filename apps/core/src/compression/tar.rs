use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};

use tar::{Archive, Builder, EntryType};

use super::{CompressionError, NamePlan, Verify};

/// The sentence at the bottom of tar's error chain.
///
/// `unpack_in` returns a stack of wrappers, each adding the destination again:
///
/// ```text
/// [0] failed to unpack `/…/out/a.txt/b.txt`
/// [1] failed to unpack `a.txt/b.txt` into `/…/out/a.txt/b.txt`
/// [2] Not a directory (os error 20)
/// ```
///
/// Only the last says what actually went wrong, and it is the register zip and
/// 7z answer in ("File exists (os error 17)"). Wrapping level 0 in a
/// [`CompressionError::Entry`], which names the destination itself, would print
/// the path three times.
fn root_cause(error: io::Error) -> io::Error {
    let kind = error.kind();
    let message = {
        let mut deepest: &(dyn std::error::Error + 'static) = &error;
        while let Some(next) = deepest.source() {
            deepest = next;
        }
        deepest.to_string()
    };
    io::Error::new(kind, message)
}

/// tar is an archive container without compression, so there is no level.
pub fn compress_tar(source: &Path, output: &Path, arcname: &str) -> Result<(), CompressionError> {
    // Open the source before creating the output so a missing source
    // does not leave an empty archive behind.
    let mut source_file = File::open(source)?;

    let output_file = File::create(output)?;
    let mut builder = Builder::new(output_file);

    builder
        .append_file(arcname, &mut source_file)
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    builder
        .finish()
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    Ok(())
}

/// Archive a whole directory tree into a standard tar file.
///
/// Entries are stored relative to (and prefixed with) the directory's own
/// name, so `photos/` yields `photos/a.jpg`, `photos/sub/b.jpg`, … — the same
/// layout `tar` and other tools produce, and round-trips back via `extract`.
///
/// Driven by the shared [`walk_tree`](super::walk_tree), so symlinks are
/// skipped (same as zip/7z) — the archive never carries a symlink that could
/// point outside the tree.
pub fn compress_tar_dir(source_dir: &Path, output: &Path) -> Result<(), CompressionError> {
    let entries = super::walk_tree(source_dir)?;

    let output_file = File::create(output)?;
    let mut builder = Builder::new(output_file);

    for entry in entries {
        if entry.is_dir {
            builder
                .append_dir(&entry.archive_name, &entry.disk_path)
                .map_err(|e| CompressionError::Failed(e.to_string()))?;
        } else {
            let mut file = File::open(&entry.disk_path)?;
            builder
                .append_file(&entry.archive_name, &mut file)
                .map_err(|e| CompressionError::Failed(e.to_string()))?;
        }
    }

    builder
        .finish()
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    Ok(())
}

/// Read a tar back for [`verify_archive`](super::verify_archive), returning the
/// names it holds.
///
/// Both depths walk every header, and the `tar` crate checks each header's
/// `cksum` field as it goes, so a bent header is caught either way; both also
/// have to move over the data to reach the next header, which is what catches a
/// member cut short.
///
/// [`Verify::Contents`] additionally reads each entry's bytes into a sink, and
/// it is worth being honest about what that adds: **tar stores no checksum over
/// an entry's data**, only over the 512 byte header, so this confirms the data
/// is all there and readable and cannot confirm it is unchanged. zip and 7z get
/// a real per-entry CRC here; tar cannot, from any reader.
pub(crate) fn read_tar_entries(
    archive: &Path,
    depth: Verify,
) -> Result<Vec<String>, CompressionError> {
    let file = File::open(archive)?;
    let mut ar = Archive::new(file);
    let entries = ar.entries().map_err(|e| {
        CompressionError::Failed(format!("the archive could not be read back: {e}"))
    })?;

    let mut names = Vec::new();
    for entry in entries {
        let mut entry = entry.map_err(|e| {
            CompressionError::Failed(format!("the archive could not be read back: {e}"))
        })?;
        let name = entry
            .path()
            .map_err(|e| CompressionError::Failed(format!("an entry has an unreadable name: {e}")))?
            .to_string_lossy()
            .to_string();
        if depth == Verify::Contents {
            io::copy(&mut entry, &mut io::sink()).map_err(|e| {
                CompressionError::Failed(format!("entry {name:?} could not be read back: {e}"))
            })?;
        }
        names.push(name);
    }
    Ok(names)
}

/// The names a tar holds that extraction would write, walking the headers and
/// skipping the data.
///
/// Filtered the same way [`extract_tar`] filters, so a caller asking what an
/// archive contains is never told about an entry that would be skipped. It
/// seeks past each member rather than reading it, which is what keeps a listing
/// cheap on a large archive; [`read_tar_entries`] deliberately does not, since
/// reading every byte is half of what verification is for.
pub(crate) fn list_tar_entries(archive: &Path) -> Result<Vec<String>, CompressionError> {
    let file = File::open(archive)?;
    let mut ar = Archive::new(file);
    let entries = ar
        .entries_with_seek()
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| CompressionError::Failed(e.to_string()))?;
        let entry_type = entry.header().entry_type();
        if entry_type != EntryType::Regular && entry_type != EntryType::Directory {
            continue;
        }
        names.push(
            entry
                .path()
                .map_err(|e| CompressionError::Failed(e.to_string()))?
                .to_string_lossy()
                .to_string(),
        );
    }
    Ok(names)
}

pub fn extract_tar(archive: &Path, output_dir: &Path) -> Result<Vec<String>, CompressionError> {
    extract_tar_planned(archive, output_dir, &NamePlan::identity())
}

/// [`extract_tar`], writing each entry under the name `plan` gives it.
///
/// Two write paths, and the split is deliberate. `unpack_in` derives the
/// destination from the entry's own name, so it cannot write a renamed entry at
/// all; but it is also the traversal guard tar has always used, and the
/// canonicalizing containment check inside it is what stops a write from
/// following a symlink that was already sitting in the output directory. So an
/// entry whose name is unchanged still goes through it, exactly as before, and
/// only a renamed one is unpacked to an explicit destination, with the same
/// containment check made here.
pub(crate) fn extract_tar_planned(
    archive: &Path,
    output_dir: &Path,
    plan: &NamePlan,
) -> Result<Vec<String>, CompressionError> {
    fs::create_dir_all(output_dir)?;
    let canonical_output = output_dir.canonicalize()?;

    let file = File::open(archive)?;
    let mut ar = Archive::new(file);

    let mut extracted = Vec::new();
    let entries = ar
        .entries()
        .map_err(|e| CompressionError::Failed(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| CompressionError::Failed(e.to_string()))?;
        let name = entry
            .path()
            .map_err(|e| CompressionError::Failed(e.to_string()))?
            .to_string_lossy()
            .to_string();

        // Only ever materialize regular files and directories. Symlinks,
        // hardlinks and special nodes are skipped so extraction never plants
        // an outbound link in the output tree, the same "no links" guarantee
        // zip/7z give (they write link entries as regular files). Checked
        // before anything else about the name, so an entry that is not going to
        // be written is not judged either.
        let entry_type = entry.header().entry_type();
        if entry_type != EntryType::Regular && entry_type != EntryType::Directory {
            continue;
        }

        // The path unpack_in would write to: its `Normal` components, with a
        // root or a drive stripped. `..` is refused here rather than left to
        // unpack_in's `Ok(false)`, because the renamed branch below never calls
        // unpack_in and would otherwise have no guard at all.
        let natural = normal_path(&name).ok_or_else(|| {
            CompressionError::Failed(format!("Path traversal detected in archive entry: {name}"))
        })?;
        if natural.as_os_str().is_empty() {
            // Nothing but `.` or a root: unpack_in treats it as an empty name
            // and writes nothing, and neither do we.
            continue;
        }

        match plan.written_as(&name) {
            None => {
                // `unpack_in` derives the destination itself, so the failure it
                // reports names a path and nothing else: `failed to unpack
                // \`/…/out/a.txt/b.txt\``, with no clue which of an archive's
                // entries was at fault. That is what issue #64 was about, and
                // the fix reached zip and 7z but not this branch, which is the
                // one nearly every archive takes (issue #93).
                //
                // The call itself is untouched. It is the traversal guard tar
                // has always used, and its canonicalizing containment check is
                // what stops a write from following a symlink already sitting
                // in the output directory. Only its error is dressed.
                let unpacked = entry.unpack_in(&canonical_output).map_err(|e| {
                    super::entry_error(&name, &canonical_output.join(&natural), root_cause(e))
                })?;
                if !unpacked {
                    return Err(CompressionError::Failed(format!(
                        "Path traversal detected in archive entry: {name}"
                    )));
                }
                if entry_type == EntryType::Regular {
                    extracted.push(natural.to_string_lossy().to_string());
                }
            }
            Some(rel) => {
                let dest = canonical_output.join(rel);
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(|e| super::entry_error(&name, &dest, e))?;
                    // What unpack_in's `validate_inside_dst` does: resolve the
                    // directory being written into and refuse one that turned
                    // out to be somewhere else.
                    let resolved = parent
                        .canonicalize()
                        .map_err(|e| super::entry_error(&name, &dest, e))?;
                    if !resolved.starts_with(&canonical_output) {
                        return Err(CompressionError::Failed(format!(
                            "Path traversal detected in archive entry: {name}"
                        )));
                    }
                }
                entry
                    .unpack(&dest)
                    .map_err(|e| super::entry_error(&name, &dest, e))?;
                if entry_type == EntryType::Regular {
                    extracted.push(rel.to_string_lossy().to_string());
                }
            }
        }
    }
    Ok(extracted)
}

/// An entry's path reduced to the components tar would write: `Normal` ones
/// only, a root or drive prefix dropped, `.` dropped, and `None` for any `..`,
/// which is the traversal `unpack_in` refuses.
fn normal_path(name: &str) -> Option<PathBuf> {
    let mut safe = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::ParentDir => return None,
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    Some(safe)
}
