# Architecture

Collapse is a small file-compression toolkit built around **one shared engine**
with thin interfaces on top. Everything is a Cargo workspace under `apps/`, with
a mirrored test tree under `tests/`.

This document describes what exists today: the `collapse-core` engine and the
`collapse` CLI. Interfaces still to come (desktop app) are noted at the end.

## Workspace layout

```
apps/
  core/        collapse-core — the shared compression/extraction engine
  cli/         collapse-cli  — the `collapse` command-line tool (lib + bin)
tests/
  core/        collapse-core-tests — integration tests for apps/core
  cli/         collapse-cli-tests  — integration tests for apps/cli
docs/          architecture.md, security.md, git_flow.md
```

Each app under `apps/` has a matching test crate under `tests/` (see
[Testing](#testing)). Adding an app means adding **both** to `members` in the
root `Cargo.toml`.

## Dependency graph

```
collapse-core  ◄──  collapse-cli
      ▲                  ▲
      │                  │
collapse-core-tests   collapse-cli-tests
```

Everything flows from `collapse-core`. Interfaces call it directly as a Rust
library — there is no HTTP, IPC, or subprocess boundary between an interface and
the engine. New interfaces depend on `collapse-core`; they never reach into each
other.

## collapse-core — the engine

A single-responsibility library: compress a file or a directory into an archive,
and extract an archive back out. Two formats compress (7z via `sevenz-rust2`,
ZIP via `zip`); tar (via `tar`) is an uncompressed container. `src/lib.rs`
re-exports the public surface: `compress`, `compress_dir`, `extract`,
`Algorithm`, `CompressionError`.

### Modules (`src/compression/…`)

| Module | Responsibility |
|--------|----------------|
| `compression.rs` | Public dispatchers (`compress`, `compress_dir`, `extract`), the `CompressionError` type, and the `sanitize_entry_path` path-traversal guard. |
| `compression/algorithm.rs` | The `Algorithm` enum (`SevenZ`, `Tar`, `Zip`) and its `extension()` / `media_type()` / `from_extension()` / `FromStr` / `Display`. |
| `compression/walk.rs` | `walk_tree` — the shared, symlink-skipping directory walker that turns a folder into a deterministic list of entries. |
| `compression/sevenz.rs` | 7z backend: `compress_7z`, `compress_7z_dir`, `extract_7z`. |
| `compression/zip.rs` | ZIP backend: `compress_zip`, `compress_zip_dir`, `extract_zip`. |
| `compression/tar.rs` | tar backend: `compress_tar`, `compress_tar_dir`, `extract_tar`. |

### The `Algorithm` enum is the single extension point

`compress`/`compress_dir`/`extract` are dispatchers keyed on `Algorithm`. To add
a Rust-side format you touch **only** core: add an enum variant, create
`compression/<name>.rs` with the `compress_<name>` / `compress_<name>_dir` /
`extract_<name>` functions, and wire the match arms. Interfaces pick it up
automatically because they go through the same dispatchers.

### Behavioral contract

- **Level.** Public level is `1`–`5`, mapped to per-format presets `[1, 3, 5, 7, 9]`.
  Validation lives **only** in the `compress`/`compress_dir` dispatchers; tar
  ignores the level but still requires it in range.
- **Single file vs. directory.** `compress` stores one file under a caller-given
  `arcname`; `compress_dir` archives a whole tree, with entries prefixed by the
  folder's own name (the `tar` / `zip -r` convention). All three backends share
  `walk_tree`, so all three skip symlinks.
- **Extraction.** `extract` detects the format from the archive's **file
  extension** (not magic bytes) and recreates the full tree, handling nested
  directories and directory entries.
- **Errors.** `CompressionError::{Io, Failed, InvalidLevel}` (via `thiserror`);
  third-party crate errors are stringified into `Failed`.

### Security

Extraction is hardened against path traversal ("ZIP Slip"), and neither
compression nor extraction ever follows or creates a symlink. The full threat
model, the measures, and the attacks they prevent are documented in
[security.md](security.md).

## collapse-cli — the command-line tool

`collapse-cli` is a **library + binary** (binary name `collapse`). The split is
deliberate:

- `src/lib.rs` holds all the logic: the clap types (`Cli`, `Command`, `Format`),
  and `run(cli) -> Result<Outcome, CliError>`.
- `src/main.rs` is a shell: parse args, call `run`, print the `Outcome`, and map
  any `CliError` to a stderr message + exit code 1.

Keeping the logic in the lib lets the test crate drive the **real clap parser**
in-process (`Cli::try_parse_from([...])` then `run(cli)`) and assert filesystem
effects — no subprocess needed.

### Command surface

```
collapse compress <file|dir> [-f 7z|zip|tar] [-l 1-5] [-o <path>] [--force]
collapse extract  <archive>  [-o <dir>]
```

Aliases `c` / `e`. The CLI-local `Format` enum (`clap::ValueEnum`) converts to
`collapse_core::Algorithm`, which keeps `clap` out of `collapse-core`.

### Compression flow (`run_compress`)

1. **Canonicalize the source** (so `.`, `..`, and trailing slashes resolve to a
   real path with a usable name; also lets the safety check detect a source-alias).
2. **Resolve the format**: explicit `--format` wins; otherwise infer it from the
   output file's extension; otherwise default to zip.
3. **Determine the output path**: `-o` if given, else `<source>.<ext>` beside the
   source.
4. **Safety guards** (before writing anything):
   - refuse if the output would overwrite its **own source** (this would truncate
     the source before it is read — data loss);
   - refuse an existing output unless `--force`;
   - reject a source that is neither a regular file nor a directory (e.g. a FIFO).
5. Dispatch to `compress_dir` (directory) or `compress` (file).

Extraction (`run_extract`) resolves the output directory (default the current
directory) and calls `collapse_core::extract`, which creates the directory tree
as needed.

## Testing

Tests live in the **mirrored `tests/` tree**, not next to the source:

- `tests/core` exercises `collapse-core` through its public API (`compress`,
  `extract`, and the backend functions), including a dedicated
  `security.rs` suite that crafts malicious archives.
- `tests/cli` drives the real clap parser and `run` in-process.

Because tests live in separate crates, they only see each crate's **public**
surface — anything a test needs must be reachable from outside. Source files in
`apps/` carry no inline `#[cfg(test)] mod tests`.

## CI

`.github/workflows/tests.yml` runs `cargo test` for the whole workspace on every
push to `main`/`dev` and on pull requests — which compiles and tests both
`collapse-core` and `collapse-cli`. Branching and release flow are described in
[git_flow.md](git_flow.md).

## Planned components

The MVP targets two interfaces; the desktop app is the remaining one:

- **Desktop app** (Tauri v2, Vue frontend + Rust backend calling `collapse-core`
  directly) — tracked in the repository issues.

Because every interface funnels through `collapse-core`, the desktop app inherits
the same formats, directory support, and security guarantees for free; it only
adds a UI and its own thin command layer.
