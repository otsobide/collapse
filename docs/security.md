# Security

This document describes the security measures implemented in Collapse, the
attacks they prevent, and the current limitations. Everything here applies to
**`collapse-core`** — the engine every interface (CLI, desktop, …) builds on —
so the guarantees hold no matter which front-end invokes it.

## Threat model

The dangerous input is an **archive you did not create**: extracting an
untrusted `.zip`, `.7z`, or `.tar` must never let the archive write, link, or
read outside the directory you chose as the extraction target. Compression is
lower-risk (you own the input tree), but it must not silently pull data from
**outside** that tree into the archive.

The trust boundary is the **output directory** on extraction, and the
**source directory** on compression. Every measure below defends one of those
boundaries.

---

## Extraction measures

### 1. Path traversal ("ZIP Slip")

**Attack.** An archive entry named `../../etc/cron.d/evil` or `/etc/passwd`
tries to make the extractor write outside the output directory by walking up
with `..` or by using an absolute path. This is the classic *ZIP Slip* /
*tar traversal* vulnerability (CWE-22).

**Prevention.**

- **zip & 7z** share `sanitize_entry_path` (`apps/core/src/compression.rs`).
  For every entry it reduces the name to its `Normal` path components and
  returns *nothing* — aborting the extraction — if the name contains any
  `..` (`ParentDir`), a root (`/`, `RootDir`), a drive/UNC prefix, or reduces
  to empty (`.`, ``, `/`). The check runs **before any byte touches disk**, and
  the destination is always `output_dir.join(clean_relative)`, which cannot
  escape. This deliberately replaces a weaker "resolve then compare" check that
  failed open when the target file did not yet exist.
- **tar** delegates to the `tar` crate's `unpack_in`, which refuses any entry
  with a `..` component (Collapse turns that refusal into an error) and strips
  root/current-dir components so absolute names land *inside* the output dir.

**Covered by** `tests/core/tests/security.rs`:
`*_rejects_parent_dir_traversal`, `*_rejects_nested_parent_dir_traversal`,
`*_rejects_absolute_path`, `*_rejects_directory_entry_traversal` (traversal via
a *directory* entry, not just a file), `*_rejects_malicious_entry_after_benign_ones`
(a bad entry after good ones still aborts the whole run with nothing escaping),
and `dispatch_extract_rejects_traversal_for_every_format`.

### 2. Symlink write-through escape

**Attack.** An archive contains a symlink entry (e.g. `link → ..` or
`link → /tmp`) followed by a file entry `link/evil` — a naive extractor creates
the link and then writes *through* it, landing `evil` outside the output dir.

**Prevention.** Collapse **never creates a symlink on extraction**:

- **zip & 7z** write a symlink entry as an ordinary regular file (its content
  is the link text), so there is no link to write through.
- **tar** skips any entry that is not a regular file or a directory (symlinks,
  hardlinks, and special nodes are dropped). A follow-up file whose parent was
  the skipped link is created as a real subdirectory *inside* the output dir
  instead. `unpack_in` additionally canonicalizes each entry's parent and
  blocks writing through any pre-existing symlink.

**Covered by** `tar_symlink_write_through_does_not_escape`,
`zip_symlink_entry_is_not_materialized_as_symlink`,
`tar_symlink_entry_is_not_materialized`.

### 3. Hardlink escape (tar)

**Attack.** A tar hardlink entry whose target is `../victim` or an absolute
path outside the output dir.

**Prevention.** `extract_tar` skips hardlink entries entirely (they are not a
regular file or directory), and `unpack_in` validates any hardlink target is
inside the output dir before linking. Verified during an adversarial review
(hardlink-to-absolute, hardlink-to-`../`, hardlink-through-symlink all blocked).

### 4. Symlink planting

**Attack.** Even without an escape, an archive can drop an inert symlink
`report.pdf → /etc/shadow` inside the output tree; a later reader that follows
it discloses an outside file.

**Prevention.** Same "no links created" policy as measure #2 — no extractor
materializes a symlink, so nothing is planted.

---

## Compression measures

### 5. Symlinks are never followed out of the tree

**Attack.** A directory being archived contains a symlink pointing outside
itself (e.g. `photos/leak → /home/user/.ssh/id_rsa`). Following it would copy
an outside secret **into** the archive, or store an absolute link that resolves
to an outside file when extracted elsewhere.

**Prevention.** All three directory backends walk the tree through the shared
`walk_tree` helper (`apps/core/src/compression/walk.rs`), which inspects each
child's type with `DirEntry::file_type()` (does **not** follow links) and
**skips symlinks entirely**. No format stores a symlink, so an archive can
never carry a link that points outside the source tree. (tar previously used
`append_dir_all`, which *stored* symlinks — this was unified onto `walk_tree`
specifically to close that gap.)

**Covered by** `compress_dir_skips_symlinks_for_every_format`.

### 6. Generated entry names are traversal-free by construction

Entry names produced when compressing a directory come from real on-disk file
names, prefixed with the source directory's own name and joined with `/`. They
cannot contain `..` or absolute components, so an archive Collapse produces is
itself never a traversal vector.

### 7. Compression level validation

**Attack.** An out-of-range level could index a preset table out of bounds and
panic (a denial of service in a hosting process).

**Prevention.** `compress` and `compress_dir` validate `level` is `1..=5`
before dispatching (`CompressionError::InvalidLevel` otherwise). The per-format
backends are only ever reached with a validated level.

**Covered by** `compress_invalid_level_zero`, `compress_invalid_level_six`,
`compress_dir_invalid_level_is_rejected`.

---

## Known limitations (out of scope for now)

These are **not** currently mitigated; treat them as accepted risk for the MVP
and revisit before exposing extraction to fully untrusted, unbounded input.

- **Decompression bombs.** There is no limit on the *output* size or entry
  count of an archive, so a "zip bomb" (a tiny archive that expands to enormous
  data) can exhaust disk or memory. Extraction reads each entry fully into
  memory before writing.
- **Resource exhaustion.** The directory walker recurses without a depth cap
  (a pathologically deep tree could overflow the stack) and buffers whole files
  in memory (no streaming). Symlink loops cannot occur — symlinks are skipped.
- **Symlinks/hardlinks are not preserved.** This is a deliberate safety choice,
  but it means archiving and re-extracting a tree with links loses them (a
  fidelity limitation, not a security hole).
- **Format detection is by file extension only.** `extract` picks the backend
  from the archive's extension, not from magic bytes. A mismatched extension
  simply fails to parse; it is not a security issue, but it is not content
  sniffing.
- **TOCTOU.** There is a small time-of-check/time-of-use window between reading
  a directory entry's type and opening it during compression. It is pre-existing
  and low-risk for a local tool.
- **Platform note.** The path guards key off `std::path::Component`. On Unix a
  backslash is an ordinary filename character (so a `..\..` name stays inside
  the output dir as a single odd filename); on Windows it is a path separator
  and is parsed — and rejected — as `..`.

---

## Testing & verification

All measures above are exercised by `tests/core/tests/security.rs`, which
crafts genuinely malicious archives (smuggling traversal and symlink entries
past the writer libraries' own validation by writing raw header bytes) and
asserts both that extraction is rejected/neutralized **and** that nothing was
written outside the output directory. The path-traversal and symlink fixes were
additionally reviewed adversarially — the guards were reverted to confirm each
test fails against the vulnerable code, so the tests are real regression guards,
not vacuous.
