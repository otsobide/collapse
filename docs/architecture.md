# Architecture

Collapse is a small file-compression toolkit built around **one shared engine**
with thin interfaces on top. Everything lives under `apps/`, each app carrying
its own tests in that ecosystem's conventional place.

This document describes what exists today: the `collapse-core` engine, the
`collapse` CLI, and the Tauri desktop app.

## Workspace layout

```
apps/
  core/        collapse-core — the shared engine (src/ + tests/ integration tests)
  cli/         collapse-cli  — the `collapse` CLI, lib + bin (src/ + tests/)
  desktop/     collapse-desktop — Tauri v2 desktop app (Vue + Rust, tests/ = Vitest)
  landing/     collapse-landing — Nuxt 3 static product site (no tests; deployed by deploy-landing.yml)
docs/          architecture.md, threat_model.md, desktop.md, deployment.md, git_flow.md
```

`apps/core` and `apps/cli` are members of the **root Cargo workspace**; each keeps
its Cargo integration tests in its own `tests/` directory (the Rust convention).
`apps/desktop/src-tauri` is a **separate workspace** (empty `[workspace]` in its
`Cargo.toml`) so a plain `cargo test` at the root doesn't need the Tauri system
dependencies — see [desktop.md](desktop.md). `apps/landing` is a Node app with
no Rust at all, outside the Cargo workspace entirely — see
[deployment.md](deployment.md).

## Dependency graph

```
collapse-core  ◄──  collapse-cli
             ◄──  collapse-desktop (apps/desktop/src-tauri)
```

Everything flows from `collapse-core`. Interfaces call it directly as a Rust
library — there is no HTTP, IPC, or subprocess boundary between an interface and
the engine. New interfaces depend on `collapse-core`; they never reach into each
other. (Each crate's tests live inside it — see [Testing](#testing).)

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
[threat_model.md](threat_model.md).

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

## collapse-desktop — the desktop app

A **Tauri v2** app: a Vue 3 frontend (`src/App.vue` is the UI; `src/paths.js`
holds the path/format helpers, split out for unit testing) over a small
Rust backend (`src-tauri/src/lib.rs`) that calls `collapse-core` directly — no
HTTP, same engine as the CLI. It compresses files and folders and extracts
archives, in the cervantic visual style (warm cream + terracotta, monospace), and
targets macOS, Windows and Linux from one codebase.

The backend exposes three Tauri commands: `is_directory` (UI icon/name hint),
`compress_path` (dispatches file vs. folder, refuses to overwrite its own
source), and `extract_archive`. Every path is chosen through the native
open/save dialogs, which is also what makes the app work under the macOS App
Store sandbox. Build, signing, and per-platform distribution (including App Store
steps) are documented in [desktop.md](desktop.md); it inherits every format,
directory, and security guarantee from `collapse-core` for free.

## Testing

Each app carries its own tests, in that ecosystem's conventional place:

- `apps/core/tests/` — Cargo integration tests exercising `collapse-core` through
  its public API (`compress`, `extract`, and the backend functions), including a
  dedicated `security.rs` suite that crafts malicious archives.
- `apps/cli/tests/` — drives the real clap parser and `run` in-process.
- `apps/desktop/tests/` — Vitest suite (unit + component tests, Tauri IPC mocked).

The Rust integration tests compile as separate crates, so they only see each
crate's **public** surface — anything a test needs must be reachable from
outside. Source files carry no inline `#[cfg(test)] mod tests`.

## CI

`.github/workflows/test-and-build.yml` runs on every push to `main`/`dev` and
on pull requests, entirely on Linux runners. Per app, tests gate the build: a
`core` job (`make core/test`) gates a `cli` job and a `vitest` job (the
desktop suite, Tauri IPC mocked), and each app's build job runs only after
its own tests pass — `build (core)`, `build (cli)`, and `build (desktop)`,
the last compiling the whole Tauri app (frontend + the `src-tauri` crate,
which no other CI job compiles) via `make desktop/compile`
(`tauri build --no-bundle`), with the webkit system libraries installed on
the runner. The landing has no tests and is built by `deploy-landing.yml`;
release artifacts (macOS tarballs and `.dmg`, Linux `.deb`/`.rpm`) are
`release.yml`'s job (which also runs the Rust test suite once on a macOS
runner, one of the OSes the binaries actually target).
Branching and release flow are described in [git_flow.md](git_flow.md).

## Roadmap

The MVP interfaces — CLI and desktop app — are both in place, tested and built
in CI, and shipped by the release pipeline. Remaining work (code signing /
store submission, decompression-bomb limits) is tracked in the repository
issues.
