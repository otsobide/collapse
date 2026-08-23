# Security

This document describes the security measures implemented in Collapse, the
attacks they prevent, and the current limitations. Everything here applies to
**`collapse-core`** — the engine every interface (CLI, desktop, server) builds
on — so the guarantees hold no matter which front-end invokes it, plus a
section on what changes once a **server** is doing the work for someone else.

## Threat model

The dangerous input is an **archive you did not create**: extracting an
untrusted `.zip`, `.7z`, or `.tar` must never let the archive write, link, or
read outside the directory you chose as the extraction target. Compression is
lower-risk (you own the input tree), but it must not silently pull data from
**outside** that tree into the archive.

That framing assumes the machine doing the work owns its input, which is true
of the CLI and the desktop app. It is **not** true of `collapse-server-backend`, which
compresses what a client sends and, for directory uploads, extracts it first:
see [The API server](#the-api-server).

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

**Covered by** `apps/core/tests/security.rs`:
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
`compress_dir_invalid_level_is_rejected` (in `apps/core/tests/compression.rs`).

---

## The API server

`collapse-server-backend` (and therefore the CLI's `--server`, and any front-end that
uses `collapse-remote`) moves the trust boundary: the server acts on bytes
someone else sent it.

### 8. The server extracts an untrusted archive

**Attack.** `POST /compress?envelope=tar` hands the server a tar that it
unpacks before compressing the tree inside. That is the extraction direction,
on input the server did not create, which is exactly the dangerous case this
document opens with.

**Prevention.** The unpacking goes through the same `extract_tar` every other
caller uses, so it inherits every guarantee in measures #1 to #4: `unpack_in`
refuses entries with `..`, strips root components, and blocks writing through
a pre-existing symlink, while entries that are not regular files or
directories (symlinks, hardlinks, device nodes) are skipped rather than
created. The destination is the job's own staging directory, one per job.

On top of that, the shape of what was unpacked is checked before compressing:
it must be exactly one entry, it must be a directory, and its name must match
the `name` the job was created for. A mismatch fails the job instead of
compressing whatever happened to arrive.

**Covered by** `apps/server-backend/tests/security.rs`, which posts hostile tars at the
server itself: `a_traversing_entry_never_escapes_the_staging_area`,
`an_absolute_entry_never_escapes_the_staging_area`,
`a_symlink_entry_is_not_materialized`,
`an_envelope_that_is_not_a_directory_is_refused` and
`an_empty_envelope_is_refused`. Core's own suite already proves the extractor
holds; these prove the **server** is wired to it and that a bad envelope fails
the job rather than producing an archive.

**Choice of envelope.** Tar is used *because it does not compress*. An archive
that expands could turn a small upload into an unbounded write; a tar cannot,
so the existing `--max-upload-mb` cap also bounds what reaches the disk. A zip
or 7z envelope would have introduced a decompression bomb where there is none
today, which is why it is not offered.

### 9. What the server does not defend against

Stated plainly, because deploying it assumes these:

- **No authentication and no rate limiting.** Anyone who can reach the port can
  submit jobs, and jobs consume CPU, memory and disk. Bind it to localhost (the
  default, and what the container image publishes) or put it behind something
  that authenticates.
- **The web frontend proxies the API, so its port exposes the API too.** nginx
  forwards `/compress` and `/jobs` to the backend, which is what keeps the
  browser same-origin; the consequence is that publishing the web port to a
  network publishes the whole unauthenticated API to it as well. Both default
  to localhost for this reason. See [server.md](server.md#exposure).
- **No transport security.** The server speaks plain HTTP, so uploaded content
  and downloaded archives travel in the clear. Do not send anything sensitive
  across a network you do not trust; terminate TLS in front of it if you must.

  This cuts both ways: a **client** that picks a remote destination (the CLI's
  `--server`, the desktop app's picker) is putting the file's contents on the
  network. The desktop app says so in its servers panel, and both default to
  compressing locally.
- **Uploads are held in memory.** The request body is buffered before staging,
  and the zip/7z backends buffer whole files, so concurrent large uploads
  multiply. The upload cap and a container memory limit are the only ceilings;
  the compose file sets one for that reason.
- **Staging is only cleaned on request.** A job's files live until `DELETE
  /jobs/{id}` (or the process exits, with the default temporary directory).
  Abandoned jobs accumulate.

---

## Known limitations (out of scope for now)

These are **not** currently mitigated; treat them as accepted risk for the MVP
and revisit before exposing extraction to fully untrusted, unbounded input.

- **Decompression bombs.** There is no limit on the *output* size or entry
  count of an archive, so a "zip bomb" (a tiny archive that expands to enormous
  data) can exhaust disk or memory. The zip and 7z extractors read each entry
  fully into memory before writing (tar streams entries through `unpack_in`).
- **Resource exhaustion.** The directory walker recurses without a depth cap
  (a pathologically deep tree could overflow the stack), and the zip and 7z
  backends buffer whole files in memory (tar streams file contents). Symlink
  loops cannot occur — symlinks are skipped.
- **Symlinks/hardlinks are not preserved.** This is a deliberate safety choice,
  but it means archiving and re-extracting a tree with links loses them (a
  fidelity limitation, not a security hole).
- **Format detection is by file extension only.** `extract` picks the backend
  from the archive's extension with a case-insensitive match, not from magic
  bytes. A mismatched extension simply fails to parse; it is not a security
  issue, but it is not content sniffing.
- **TOCTOU.** There is a small time-of-check/time-of-use window between reading
  a directory entry's type and opening it during compression. It is pre-existing
  and low-risk for a local tool.
- **Platform note.** The path guards key off `std::path::Component`. On Unix a
  backslash is an ordinary filename character (so a `..\..` name stays inside
  the output dir as a single odd filename); on Windows it is a path separator
  and is parsed — and rejected — as `..`.

---

## Testing & verification

There are two security suites: `apps/server-backend/tests/security.rs` for what a client
can send the server (measure 8), and, for the engine itself,
`apps/core/tests/security.rs` (level validation, measure 7, lives with the
functional tests in `apps/core/tests/compression.rs`), which
crafts genuinely malicious archives (smuggling traversal and symlink entries
past the writer libraries' own validation by writing raw header bytes) and
asserts both that extraction is rejected/neutralized **and** that nothing was
written outside the output directory. The path-traversal and symlink fixes were
additionally reviewed adversarially — the guards were reverted to confirm each
test fails against the vulnerable code, so the tests are real regression guards,
not vacuous.
