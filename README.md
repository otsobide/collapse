# 🌟 Collapse

A small, fast, beautiful file compressor — compress **and** extract from your
terminal or a native desktop window. One shared Rust engine, two ways to use it.

Supports **7z** (LZMA2), **ZIP** (Deflate), and **tar** (uncompressed container),
with safe extraction that refuses path-traversal ("ZIP Slip") archives.

> **MVP scope.** Collapse is being built incrementally. The first release targets
> two front-ends only — a **CLI** and a **desktop app** — both powered by the
> `collapse-core` library. Other interfaces (HTTP API, web UI) are out of scope
> for now.

## Why Collapse

- **Compress and decompress** — not a one-way tool. Every supported format round-trips.
- **Files or whole folders** — archive a single file or an entire directory tree into a standard `.7z`, `.zip`, or `.tar` that any other tool can open.
- **Three formats, one interface** — pick 7z, ZIP, or tar; the level (1–5) applies
  to the compressing formats and is ignored by tar.
- **Safe by default** — extraction rejects entries with absolute paths or `..`
  components before writing anything to disk, so a malicious archive can't escape
  the output directory. See [Security](docs/security.md) for the full threat model.
- **Shared engine** — the CLI and the desktop app call the exact same core, so
  behavior is identical whichever you reach for.

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

Build and run from source:

```bash
cargo run -p collapse-cli -- compress notes.txt -f 7z
cargo build -p collapse-cli --release      # binary at target/release/collapse
```

### Desktop

A Tauri v2 desktop app (macOS / Windows / Linux) sharing the same engine as the
CLI: drag in a file or folder to compress, or an archive to extract, in a calm
interface using the cervantic palette. See [desktop.md](docs/desktop.md) for
building, signing, and per-platform distribution (including the macOS App Store).

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
- ✅ **CLI** (`collapse`) — compress files/folders and extract archives.
- ✅ **Desktop app** — Tauri v2 (macOS / Windows / Linux), compress & extract.

## Getting started

Requires **Rust 1.88+** (2021 edition).

```bash
make build             # build the Rust crates
make test              # run the full test suite (108 Rust tests + the desktop Vitest suite)
```

See the [CLI](#cli) and [Desktop](#desktop) sections above for installing and
running the apps.

## Project layout

```
apps/
  core/        collapse-core — the shared compression engine
  cli/         collapse-cli  — the `collapse` command-line tool (lib + bin)
  desktop/     collapse-desktop — Tauri v2 desktop app (Vue + Rust)
  landing/     collapse-landing — Nuxt landing page (the product site)
docs/          architecture.md, security.md, desktop.md, deployment.md, git_flow.md
```

Each app carries its own tests, in that ecosystem's conventional place: the Rust
crates keep Cargo integration tests in `apps/<crate>/tests/`; the desktop app
keeps its Vitest suite in `apps/desktop/tests/`.

## Documentation

- [Architecture](docs/architecture.md) — the engine, the CLI, the desktop app, and how they fit together.
- [Security](docs/security.md) — threat model, measures, and the attacks they prevent.
- [Desktop](docs/desktop.md) — building, signing, and per-platform distribution.
- [Deployment](docs/deployment.md) — how the landing site is built and published.
- [Git flow](docs/git_flow.md) — branching model (including deploy branches) and commits.

## Development

A root `Makefile` fans out to a `Makefile` in each app. Run `make help` for the
list; a few common ones:

```bash
make test            # run every test suite (core, cli, desktop)
make core/test       # a single app's tests (also cli/test, desktop/test)
make desktop/dev     # run an app target (here: the Tauri app in dev mode)
make fmt  make lint  # format / clippy the Rust workspace
```

Work happens on `dev`, merged into `main` per release (see
[git flow](docs/git_flow.md)). CI invokes these same `make` targets on every
push and pull request.

## License

To be determined.
