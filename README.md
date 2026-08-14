# 🌟 Collapse

A small, fast file compressor — compress **and** extract from your terminal or a
native desktop window. One shared Rust engine, two ways to use it.

Supports **7z** (LZMA2), **ZIP** (Deflate), and **tar** (uncompressed container),
with safe extraction that refuses path-traversal ("ZIP Slip") archives.

> **MVP scope.** Collapse is being built incrementally. The first release targets
> two front-ends only — a **CLI** and a **desktop app** — both powered by the
> `collapse-core` library. Other interfaces (HTTP API, web UI) are out of scope
> for now.

## Why Collapse

- **Compress and decompress** — not a one-way tool. Every supported format round-trips.
- **Three formats, one interface** — pick 7z, ZIP, or tar; the level (1–5) applies
  to the compressing formats and is ignored by tar.
- **Safe by default** — extraction rejects entries with absolute paths or `..`
  components before writing anything to disk, so a malicious archive can't escape
  the output directory.
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

### CLI — *planned*

A single binary to compress and extract from scripts and the shell:

```bash
collapse compress notes.txt --protocol 7z --level 5   # → notes.txt.7z
collapse extract  notes.txt.7z --output ./out         # → ./out/notes.txt
```

### Desktop — *planned*

A cross-platform desktop window (drag a file in, pick a format and level, save the
result) sharing the same engine as the CLI.

## Status

Collapse is at an early stage. Here's exactly what exists today:

- ✅ **`collapse-core`** — the compression/extraction engine (7z, ZIP, tar), with
  path-traversal-safe extraction. Fully tested (69 tests, including a dedicated
  security suite for malicious archives).
- 🚧 **CLI** — not started.
- 🚧 **Desktop app** — not started.

## Getting started

Requires **Rust 1.88+** (2021 edition).

```bash
cargo build            # build the workspace
cargo test             # run the full test suite (69 tests)
```

Once the front-ends land, this section will cover installing and running them.

## Project layout

```
apps/
  core/        collapse-core — the shared compression engine (the only crate today)
tests/
  core/        integration tests for collapse-core, mirroring apps/core
docs/          project documentation (see docs/git_flow.md)
```

Each app under `apps/` gets a mirrored test crate under `tests/`.

## Development

The branching model and commit conventions are documented in
[`docs/git_flow.md`](docs/git_flow.md): work happens on `dev`, and `dev` is merged
into `main` per release. CI runs `cargo test` on every push and pull request.

## License

To be determined.
