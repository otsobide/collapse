# 🌟 Collapse

A small, fast, beautiful file compressor — compress **and** extract from your
terminal or a native desktop window, on this machine or on another one. One
shared Rust engine behind every front-end.

Supports **7z** (LZMA2), **ZIP** (Deflate), and **tar** (uncompressed container),
with safe extraction that refuses path-traversal ("ZIP Slip") archives.

> **MVP scope.** Collapse is being built incrementally. The release artifacts
> are two front-ends — a **CLI** and a **desktop app** — both powered by the
> `collapse-core` library. The repo also carries **`collapse-server-backend`**, an optional
> compression server that either front-end can offload work to, and
> **`collapse-remote`**, the client they share to talk to it. The server is not
> shipped in releases yet. A web UI is out of scope for now.

## Why Collapse

- **Compress and decompress** — not a one-way tool. Every supported format round-trips.
- **Files or whole folders** — archive a single file or an entire directory tree into a standard `.7z`, `.zip`, or `.tar` that any other tool can open.
- **Three formats, one interface** — pick 7z, ZIP, or tar; the level (1–5) applies
  to the compressing formats and is ignored by tar.
- **Safe by default** — extraction rejects entries with absolute paths or `..`
  components before writing anything to disk, so a malicious archive can't escape
  the output directory. See [Security](docs/threat_model.md) for the full threat model.
- **Shared engine** — the CLI and the desktop app call the exact same core, so
  behavior is identical whichever you reach for.
- **Optional remote compression** — hand the work to another machine running
  the Collapse server, from the CLI's `--server` flag or the desktop app's
  destination picker. Same defaults, same guards, same resulting archive.

## Formats

| Format | Extension | Compresses | Extracts | Level 1–5 |
|--------|-----------|:----------:|:--------:|:---------:|
| 7z (LZMA2)     | `.7z`  | ✅ | ✅ | ✅ |
| ZIP (Deflate)  | `.zip` | ✅ | ✅ | ✅ |
| tar (container) | `.tar` | ✅ | ✅ | — (ignored) |

Level `1` is fastest, `5` is smallest; internally they map to presets `[1, 3, 5, 7, 9]`.
Extraction auto-detects the format from the file extension.

## Interfaces

### CLI

Compress a file or a whole folder, or extract an archive, from the shell:

```bash
collapse compress notes.txt              # → notes.txt.zip  (zip, level 3)
collapse compress photos/ -f 7z -l 5     # → photos.7z
collapse extract photos.7z -o ./out      # restores the tree into ./out

collapse c notes.txt                     # short aliases: c = compress, e = extract
collapse e notes.txt.zip
```

Formats: `zip` (default, or inferred from `-o`'s extension), `7z`, `tar`. Level
`1`–`5` (default `3`, ignored by tar). The archive defaults to `<source>.<ext>`
next to the source; override with `-o`. It **won't overwrite** an existing
archive (or its own source) unless you pass `--force`. Run `collapse --help` for
the full surface.

Prebuilt macOS binaries (Apple Silicon and Intel tarballs, with sha256
checksums) are attached to every
[GitHub release](https://github.com/otsobide/collapse/releases/latest):

```bash
tar -xzf collapse-v0.2.0-aarch64-apple-darwin.tar.gz
mv collapse ~/.local/bin/        # or anywhere on your PATH
```

The binaries are unsigned for now; if the browser download is quarantined,
clear it with `xattr -d com.apple.quarantine collapse`.

Or build and run from source:

```bash
cargo run -p collapse-cli -- compress notes.txt -f 7z
cargo build -p collapse-cli --release      # binary at target/release/collapse
```

#### Remote compression

Compression can be offloaded to a remote Collapse server: run `collapse-server-backend`
somewhere and point the CLI at it with `--server`. The bytes are sent over,
compressed there, and the archive is written locally, with the same defaults
and safety guards as local mode; under the hood the CLI follows the server's
job flow (queue, poll, download, delete). Folders work too: HTTP cannot carry
a directory, so one is packed into an uncompressed **tar envelope** the server
unpacks, which keeps the actual compression on the far side. Extraction stays
local.

```bash
cargo run -p collapse-server-backend -- --host 0.0.0.0 --port 8000    # on the server
collapse compress notes.txt -f 7z --server http://myserver:8000
collapse compress photos/  -f 7z --server http://myserver:8000
```

The server documents itself: open **`/docs`** for an interactive page that
describes every endpoint and can run them (upload a file, watch the job, save
the archive) with no other tool, and **`/openapi.json`** for the OpenAPI 3.1
description. Both are built into the binary and load nothing from the network,
so they work offline.

##### In Docker, with a web app

Compose brings up the server **and a web frontend** for it: the same compress
flow as the desktop app, in a browser, for people who will not install
anything.

```bash
make docker/up        # then http://localhost:8080 for the web app,
                      # or http://localhost:8000/docs for the API
make docker/aio       # the same, as a single all-in-one container
make docker/logs      # follow the logs
make docker/smoke     # build, start, run a real compression through the port, stop
make docker/down      # stop it (the job volume survives)
make docker/clean     # remove the container, its volume and the image
```

The images are defined in [apps/server-backend/Dockerfile](apps/server-backend/Dockerfile)
and [apps/server-frontend/Dockerfile](apps/server-frontend/Dockerfile), the stack
in [docker-compose.yml](docker-compose.yml); both build from the repository root,
since the backend depends on `collapse-core` by path. nginx serves the web app
and proxies the API through it, so the browser stays on one origin and the
backend needs no CORS layer. Both publish to **localhost only** by default,
matching the server's own posture (it has no authentication). Override the ports
with `COLLAPSE_PORT=9000 COLLAPSE_WEB_PORT=9090 make docker/up`.

### Desktop

A Tauri v2 desktop app (macOS / Windows / Linux) sharing the same engine as the
CLI: drag in a file or folder to compress, or an archive to extract, in a calm
interface using the cervantic palette. Compression can also be sent to a remote
`collapse-server-backend` server: the compress options carry a destination picker (this
computer by default) and the header gear manages the list of servers.

Every release ships an unsigned
**universal macOS `.dmg`** (right-click → Open on first launch), x86_64
Linux **`.deb`**, **`.rpm`** and **`.AppImage`** builds, and x64 Windows
installers (**`.msi`** and an NSIS **setup `.exe`**; SmartScreen warns on
first run until they are signed — More info → Run anyway): grab them from the
[releases page](https://github.com/otsobide/collapse/releases/latest) or
the landing site. See [desktop.md](docs/desktop.md) for building, signing, and
per-platform distribution (including the macOS App Store).

```bash
cd apps/desktop && npm install
npm run tauri dev      # dev window
npm run tauri build    # installers/bundles for the current OS
```

## Status

Collapse is at an early stage. Here's exactly what exists today:

- ✅ **`collapse-core`** — the compression/extraction engine (7z, ZIP, tar), with
  path-traversal-safe extraction and whole-folder support. Fully tested,
  including a dedicated security suite for malicious archives.
- ✅ **CLI** (`collapse`) — compress files/folders and extract archives, locally
  or on a remote server with `--server`.
- ✅ **Desktop app** — Tauri v2 (macOS / Windows / Linux), compress & extract,
  with a destination picker for remote servers.
- ✅ **API server** (`collapse-server-backend`) — optional remote compression over an
  asynchronous job flow, self-documenting at `/docs`, with a Dockerfile and a
  compose file, and its own security suite for hostile uploads. Not shipped in
  releases yet.
- ✅ **Web app** (`collapse-server-frontend`) — a Vue frontend for the server,
  shipped in the compose stack, so a browser can compress too.
- ✅ **`collapse-remote`** — the client both native front-ends use to talk to a
  server, so the exchange is written once.

## Getting started

Requires **Rust 1.88+** (2021 edition).

```bash
make build             # build the Rust crates
make test              # run every suite (224 Rust tests + 74 Vitest cases)
```

See the [CLI](#cli) and [Desktop](#desktop) sections above for installing and
running the apps.

## Project layout

```
apps/
  core/        collapse-core — the shared compression engine
  remote/      collapse-remote — client for a remote server, shared by front-ends
  cli/         collapse-cli  — the `collapse` command-line tool (lib + bin)
  server-backend/  collapse-server-backend — compression server (lib + bin)
  server-frontend/ collapse-server-frontend — Vue web app for that server
  server-aio/      both of the above in one container (packaging only)
  desktop/     collapse-desktop — Tauri v2 desktop app (Vue + Rust)
  landing/     collapse-landing — Nuxt landing page (the product site)
docs/          architecture.md, threat_model.md, desktop.md, deployment.md, git_flow.md
```

Each app carries its own tests, in that ecosystem's conventional place: the Rust
crates keep Cargo integration tests in `apps/<crate>/tests/`; the desktop app
keeps its Vitest suite in `apps/desktop/tests/`.

## Documentation

- [Architecture](docs/architecture.md) — the engine, the CLI, the server, the desktop app, and how they fit together.
- [Security](docs/threat_model.md) — threat model, measures, and the attacks they prevent.
- [Server](docs/server.md) — running the compression server and its web app.
- [Desktop](docs/desktop.md) — building, signing, and per-platform distribution.
- [Deployment](docs/deployment.md) — how the landing site is built and published.
- [Git flow](docs/git_flow.md) — branching model (including deploy branches) and commits.

## Development

A root `Makefile` fans out to a `Makefile` in each app. Run `make help` for the
list; a few common ones:

```bash
make test            # run every test suite (all seven apps)
make core/test       # a single app's tests (also remote/test, cli/test,
                     # server-backend/test, server-frontend/test, desktop/test)
make desktop/dev     # run an app target (here: the Tauri app in dev mode)
make fmt  make lint  # format / clippy the Rust workspace
```

Work happens on `dev`, merged into `main` per release (see
[git flow](docs/git_flow.md)). CI invokes these same `make` targets on every
pull request and on pushes to `dev` and `main`.

## License

Collapse is free software, released under the
[GNU General Public License v3.0](LICENSE).
