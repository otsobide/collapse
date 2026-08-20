# Architecture

Collapse is a small file-compression toolkit built around **one shared engine**
with thin interfaces on top. Everything lives under `apps/`, each app carrying
its own tests in that ecosystem's conventional place.

This document describes what exists today: the `collapse-core` engine, the
`collapse` CLI, the `collapse-server-backend` server and the `collapse-remote` client that
lets a front-end offload work to it, and the Tauri desktop app.

## Workspace layout

```
apps/
  core/        collapse-core — the shared engine (src/ + tests/ integration tests)
  remote/      collapse-remote — client for a remote server, shared by the front-ends
  cli/         collapse-cli  — the `collapse` CLI, lib + bin (src/ + tests/)
  server-backend/  collapse-server-backend — HTTP compression server, lib + bin (src/ + tests/)
  server-frontend/ collapse-server-frontend — Vue web app for that server (src/ + tests/ = Vitest)
  server-aio/      the two above packaged into one container (a Dockerfile, no code)
  desktop/     collapse-desktop, the Tauri v2 desktop app (Vue + Rust;
               tests/ = Vitest, src-tauri/tests/ = Cargo integration tests)
  landing/     collapse-landing — Nuxt 3 static product site (no tests; deployed by deploy-landing.yml)
docs/          architecture.md, threat_model.md, server.md, desktop.md, deployment.md, git_flow.md
```

`apps/core`, `apps/remote`, `apps/cli` and `apps/server-backend` are members of the **root Cargo workspace**; each keeps
its Cargo integration tests in its own `tests/` directory (the Rust convention).
`apps/desktop/src-tauri` is a **separate workspace** (empty `[workspace]` in its
`Cargo.toml`) so a plain `cargo test` at the root doesn't need the Tauri system
dependencies — see [desktop.md](desktop.md). `apps/landing` is a Node app with
no Rust at all, outside the Cargo workspace entirely — see
[deployment.md](deployment.md).

## Dependency graph

```
collapse-core  ◄──  collapse-cli  ──►  collapse-remote ── HTTP (opt-in) ──►  collapse-server-backend
             ◄──  collapse-desktop (apps/desktop/src-tauri)                       │
             ◄─────────────────────────────────────────────────────────────────────┘
```

Everything flows from `collapse-core`. Interfaces call it directly as a Rust
library — there is no HTTP, IPC, or subprocess boundary between an interface and
the engine. New interfaces depend on `collapse-core`; they never reach into each
other.

The one networked path is **opt-in and shared**: both front-ends can hand a
compression to a `collapse-server-backend` instance, and both do it through the same
`collapse-remote` crate rather than each growing its own client. The server
calls the very same engine on its side, so a remote archive is byte-for-byte
what a local run would have produced (a test asserts exactly that). Extraction
is always local. (Each crate's tests live inside it — see [Testing](#testing).)

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
collapse compress <file|dir> [-f 7z|zip|tar] [-l 1-5] [-o <path>] [--force] [--server <URL>]
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
5. Dispatch to `compress_dir` (directory) or `compress` (file). With
   `--server <URL>`, the source is instead compressed by a remote
   [`collapse-server-backend`](#collapse-server-backend--the-compression-server) through
   [`collapse-remote`](#collapse-remote--the-client-for-a-remote-server), which
   handles files and directories alike; the archive lands at the same output
   path local mode would use. The safety guards in step 4 run before any
   network I/O. Extraction has no remote mode.

Extraction (`run_extract`) resolves the output directory (default the current
directory) and calls `collapse_core::extract`, which creates the directory tree
as needed.

## collapse-remote — the client for a remote server

A small crate holding the client side of the server's job flow:
`compress_path` uploads the bytes, polls the job until it settles, downloads
the archive and deletes the job, returning the archive bytes. It is the only
place that exchange is written.

It takes files and directories alike. HTTP carries no notion of a folder, so a
directory is packed into a **tar envelope** first and the server is told to
unwrap it (`envelope=tar`). Tar is the envelope precisely because it does *not*
compress: the CPU work still happens on the server, and because the envelope
cannot expand, the server's upload cap also bounds how much it can unpack. The
price is bandwidth, since what travels is uncompressed.

It exists as its own crate rather than living inside the CLI because more than
one front-end needs it: the CLI's `--server` flag today, the desktop app next.
`apps/desktop/src-tauri` is a separate Cargo workspace, but a path dependency
crosses that boundary fine.

The split inside mirrors the one the rest of the project uses for testability:
`protocol.rs` is **public and pure** (URL normalization, reading the server's
JSON, and `progress_of`, which decides whether a status means keep polling,
download, or give up), while the HTTP plumbing in `client.rs` stays private.
Errors are a `RemoteError` of its own, so the crate does not depend on any
front-end's error type; the CLI absorbs it into `CliError`.

## collapse-server-backend — the compression server

`collapse-server-backend` (`apps/server-backend`) lets a client compress on another machine; it is
what the CLI's `--server` flag talks to, and it is compression-only
(extraction stays client-side). The flow is **asynchronous, job-based**,
following the reference implementation's process: uploading answers
immediately while a background worker compresses (a single queue consumer, so
concurrent uploads line up instead of oversubscribing the CPU), and the
client polls the job, downloads the archive, then deletes the job. Jobs are
staged on disk with one directory per job under the staging dir (deleting a
job is a single `remove_dir_all`) and tracked in a **SQLite registry** that
outlives the process, so a client can keep polling, downloading and above all
deleting across a restart.

The two live in separate subdirectories of the storage directory,
`registry/jobs.db` and `jobs/<job_id>/`, because they behave nothing alike: a
few kilobytes written constantly against gigabytes written once. An operator
can mount one volume over the parent or one over each. It also means
everything under `jobs/` is a job, so the sweep cannot reach the database by
mistake.

The database carries a schema version (`PRAGMA user_version`), migrated
forward on open and **refused if it is newer than this build understands**, and
each row records the build that wrote it. Nothing that deletes a job reads it
first, so a row this build cannot interpret (a format a newer version knows)
fails that one job's reads and stops nothing else. See
[registry.md](registry.md).

Because both stores can be interrupted, `build_app` **reconciles** them before
serving (`maintenance::reconcile`): jobs still `queued` or `compressing` are
failed with a reason, since no worker survived to run them; rows whose files
are gone are dropped; and directories no job claims are removed. That last one
is what used to accumulate forever. The walk treats every *directory* in the
staging area as a job, which is precisely why the database is a file next to
them.

A background **reaper** (`maintenance::reap`, swept at a tenth of the window on
a blocking task) covers the other half: finished jobs nobody downloads again
within `--job-ttl-minutes` are deleted, row and files together. Downloading
touches a job and restarts its window; polling does not, and jobs the worker
still owns are never collected. Between the two, disk is bounded by the last
window's jobs plus whatever is running.

Like the CLI it is a **library + binary**: `build_app()` (routes, state and
the worker) lives in the lib, which is what its tests, and the CLI's
end-to-end tests, drive in-process; `main.rs` only parses `--host` / `--port`
/ `--max-upload-mb` / `--storage-dir`, installs the logger and serves. It binds
`127.0.0.1` by default, and the default staging dir is a temporary directory
removed when the server stops.

A stop (SIGTERM, or Ctrl+C) is graceful: `main.rs` hands axum a shutdown future
so it stops accepting connections and finishes the requests it already has,
with `--shutdown-grace-seconds` as the deadline for a client that stops reading.
The worker is deliberately not waited for, since a compression can outlast any
container runtime's patience; a job caught mid-flight is resolved to `failed` by
the next startup's reconciliation. The clean exit is also what lets the default
staging TempDir's guard run.

It logs through `tracing` (`logging.rs` sets up the subscriber, `RUST_LOG`
picks the level): one line per HTTP request from `tower-http`'s `TraceLayer`,
plus the job lifecycle from the handlers and the worker, every line tagged with
`job=<id>`. `GET /health` is routed outside the traced layer on purpose, since
a container probe every ten seconds would drown the rest.

It also ships a container image (`apps/server-backend/Dockerfile`, plus the root
`docker-compose.yml` and the `make docker/*` targets). Two details there are
load-bearing: the build context is the **repository root**, because the crate
depends on `collapse-core` through a path dependency and cargo needs every
workspace member's manifest; and `--host 0.0.0.0` is baked into the image's
`ENTRYPOINT` rather than left to the operator, since the server's loopback
default would make a published port reach nothing from the host.

The surface:

- `GET /docs` — interactive documentation, the role FastAPI's `/docs` plays.
  It renders itself from `/openapi.json`, so a new endpoint in the document
  documents itself, and it can execute every call (file picker included),
  plus run the whole job flow end to end. Unlike FastAPI's default, which
  pulls Swagger UI from a CDN, the page is embedded in the binary with
  `include_str!` and loads **nothing** from the network, so it works on an
  offline or air-gapped host. A test asserts that invariant.
- `GET /openapi.json` — the OpenAPI 3.1 document (`apps/server-backend/assets/openapi.json`,
  hand-written; `info.version` is substituted from `CARGO_PKG_VERSION` so it
  cannot drift from the crate). Point Swagger UI, Postman or a client
  generator at it if you prefer.
- `GET /health` — liveness probe, returns `{"status":"ok"}`.
- `POST /compress?name=<file>[&algorithm=7z|tar|zip][&level=1-5][&envelope=none|tar]`
  — the body is the raw file content; answers **202 Accepted** with the queued
  job as JSON (`job_id`, `status`, `archive_name`, …). Defaults mirror the CLI
  (zip, level 3). With `envelope=tar` the body is instead a tar holding one
  directory, which the server unpacks and compresses as a tree; `name` is then
  the directory's own name. The flag is explicit rather than sniffed, because a
  `.tar` upload may equally be a file the caller wants compressed as itself.
  What the tar unpacks to is validated (exactly one entry, a directory, named
  as the job says) before anything is compressed.
- `GET /jobs/{job_id}` — the job's current state:
  `queued` → `compressing` → `completed` | `failed` (with `error_message`
  when failed).
- `GET /jobs/{job_id}/download` — the archive bytes once completed, with the
  matching `Content-Type` and a `Content-Disposition` filename; 409 while in
  progress or after a failure.
- `DELETE /jobs/{job_id}` — drops the job and its files once downloaded; 409
  while in progress.

Errors are JSON `{"detail": "..."}` with a 4xx/5xx status. Input is validated,
never coerced: an unparseable or out-of-range `level` is a 400 (the reference
implementation silently coerced it), an unknown `algorithm` is a 400, and
`name` must be a **bare file name** (no separators, no `..`, not empty), since
it becomes the arcname inside the archive and the name a tar envelope's single
root directory is checked against.

It is deliberately **not** what keeps the staging paths safe. Every path the
server builds comes from values it chose itself: a job id it generated, and
fixed names (`input/upload`, `archive.<ext>`, `tree/`). Nothing a client sends
is a path component, so the layout holds whether or not the validation does.
Uploads beyond the configurable cap get a 413. There is no CORS layer: the
server targets non-browser clients.

## collapse-server-frontend — the web app

A Vue 3 single-page app for people who will not install anything: the same
shape as the desktop app (drop a file, pick a format and a level, watch it
happen, save the archive), except the engine is on the other end of the
network. It ships in the compose stack next to the backend.

Three things are worth knowing about it:

- **One origin, no CORS.** nginx serves the built app and proxies `/compress`,
  `/jobs`, `/health` and the documentation paths to the backend, so every
  request the browser makes is same-origin. That is what lets the backend keep
  no CORS layer, rather than opening it up the way the reference implementation
  did. Vite's dev server proxies the same paths, so development matches
  production.
- **Folders are tarred in the browser.** HTTP carries no directory, and the
  backend already unwraps a tar envelope, so `src/tar.js` writes one by hand
  from a `webkitdirectory` selection. It implements enough ustar to be correct,
  including the `prefix` field for paths past 100 bytes, and its output is
  verified against the real Rust extractor.
- **The job flow is the interface.** A local run can only show a spinner; here
  every state the worker passes through is listed with the time it took, which
  is the one thing a server does that a desktop app cannot show.

Extraction is **not** offered: the backend compresses only, so there is nothing
to call. Adding it would mean a new endpoint and a new answer to what
"extracting" means when the result is a tree and the client is a browser.

## collapse-desktop — the desktop app

A **Tauri v2** app: a Vue 3 frontend (`src/App.vue` is the UI; `src/paths.js`
holds the path/format helpers, split out for unit testing) over a small
Rust backend (`src-tauri/src/lib.rs`) that calls `collapse-core` directly — no
HTTP, same engine as the CLI. It compresses files and folders and extracts
archives, in the cervantic visual style (warm cream + terracotta, monospace), and
targets macOS, Windows and Linux from one codebase.

The backend exposes four Tauri commands: `is_directory` (UI icon/name hint),
`compress_path` (dispatches file vs. folder, refuses to overwrite its own
source, and hands the work to a remote server when one is chosen),
`extract_archive`, and `check_server` (a health probe for the settings panel).
Remote work goes through [`collapse-remote`](#collapse-remote--the-client-for-a-remote-server)
from Rust rather than the webview, so the app's CSP stays `default-src 'self'`. Every path is chosen through the native
open/save dialogs, which is also what makes the app work under the macOS App
Store sandbox. Build, signing, and per-platform distribution (including App Store
steps) are documented in [desktop.md](desktop.md); it inherits every format,
directory, and security guarantee from `collapse-core` for free.

## Testing

Each app carries its own tests, in that ecosystem's conventional place:

- `apps/core/tests/` — Cargo integration tests exercising `collapse-core` through
  its public API (`compress`, `extract`, and the backend functions), including a
  dedicated `security.rs` suite that crafts malicious archives.
- `apps/remote/tests/` — `protocol.rs` unit-tests the pure helpers (URL
  building, response parsing, the poll decision) with no server involved;
  `client.rs` serves a real `collapse-server-backend` in-process to cover what no
  consumer's own suite reaches, namely the health probe and the mapping of a
  server rejection.
- `apps/cli/tests/` — drives the real clap parser and `run` in-process;
  `tests/remote.rs` serves the real `collapse-server-backend` app on an ephemeral port to
  go through remote mode end-to-end.
- `apps/server-backend/tests/` — one file per source module (`validate`, `models`,
  `registry`, `storage`, `error`, `openapi`, `logging`, `maintenance`) plus two
  cross-cutting suites.
  `api.rs` drives the whole app in-process (tower `oneshot`, no sockets)
  through the full job flow and verifies round-trips by feeding the downloaded
  bytes back through the core extractors. `security.rs` posts hostile tar
  envelopes and asserts nothing escapes a job's staging directory. The
  `openapi.rs` suite is there to stop the hand-written document drifting from
  the server: every documented path must really be routed, every documented
  enum value accepted, and every documented default the real one. The
  building-block modules are `pub` for the same reason core's backends are:
  the test crate can only see the public surface.
- `apps/server-frontend/tests/` — Vitest suite: the pure helpers (the tar
  writer, the formatters, the poll decision) plus a component test that mounts
  the app with `fetch` stubbed and drives a folder selection, which is the one
  path a real browser cannot be made to simulate.
- `apps/desktop/tests/` — Vitest suite (Tauri IPC mocked): `paths` and
  `sources` cover the pure helpers, including what the app remembers between
  launches, and `App` mounts the component to check the IPC payloads and the
  destination picker.
- `apps/desktop/src-tauri/tests/`, the Rust half, which used to have no tests
  at all: `paths` hammers the `same_file` guard from both argument orders
  (spellings, symlinks, hardlinks, paths that do not exist yet), `commands`
  drives the command surface against real files and asserts the effect on disk
  rather than only the `Result`, `remote` runs the whole remote flow against a
  real server backend served in-process, and `ipc` reads `App.vue`, `lib.rs`
  and the Vitest stub to prove the three stay in lockstep, since nothing type
  checks that crossing.

The Rust integration tests compile as separate crates, so they only see each
crate's **public** surface — anything a test needs must be reachable from
outside. Source files carry no inline `#[cfg(test)] mod tests`.

## CI

`.github/workflows/test-and-build.yml` runs on every push to `main`/`dev` and
on pull requests, entirely on Linux runners. Every job is named
`test (<app>)` or `build (<app>)`, so a check's name says what it does and
which app it belongs to; the job ids match those names (`test-cli`,
`build-cli`). Per app, tests gate the build: `test (core)` (`make core/test`)
gates `test (remote)`, `test (cli)`, `test (server-backend)` and
`test (desktop)` (the Tauri IPC is mocked, so that one needs Node only), while
`test (server-frontend)` is independent of the Rust engine and runs on its
own. Each app's build job runs only after its own tests pass: `build (core)`,
`build (remote)`, `build (cli)`, `build (server-backend)`,
`build (server-frontend)` and `build (desktop)`, the last compiling the whole
Tauri app (frontend + the `src-tauri` crate, which no other CI job compiles)
via `make desktop/compile` (`tauri build --no-bundle`), with the webkit system
libraries installed on the runner. The landing has no tests and is built by
`deploy-landing.yml`; release artifacts (macOS tarballs and `.dmg`, Linux
`.deb`/`.rpm`/`.AppImage`, Windows `.msi` and setup `.exe`) are `release.yml`'s
job (which also runs the Rust test suite once on a macOS runner, one of the
OSes the binaries actually target). Renaming a job changes its check name, so
any branch ruleset that requires the old name silently stops being satisfied.
Branching and release flow are described in [git_flow.md](git_flow.md).

## Roadmap

The MVP interfaces — CLI and desktop app — are both in place, tested and built
in CI, and shipped by the release pipeline. Remote compression (the server, the
shared client, and both front-ends' entry points) works end to end but the
server binary is not a release artifact yet. Remaining work (code signing /
store submission, decompression-bomb limits, an upper bound on how long a
client waits for a job, authentication and TLS for the server) is tracked in
the repository issues.
