# Git flow

This repository follows a **gitflow** branching model, kept deliberately simple.

## Source branches

Where humans commit code:

- **`main`** — released versions only. Every merge into `main` marks a version and gets a tag (`vX.Y.Z`). Changes arrive only through pull requests (see [Branch protections](#branch-protections)).
- **`develop`** — the integration branch where real day-to-day work happens. All work lands here first, always through a pull request from a gitflow branch; direct pushes are blocked.
- **`feature/<name>`** — branched off `develop` for each unit of work, merged back into `develop` via pull request when done. Since direct pushes to `develop` are blocked, even small changes travel on a short-lived feature branch.
- **`release/<version>`** — the version bump and any last touches before a
  release, branched off `develop` and merged back into `develop` by pull request, after
  which `develop` goes to `main` as usual. Mechanically a feature branch; the name
  is what makes CI run the macOS and Windows suites on it, which is the point of
  using it for a release rather than `feature/release-<version>`.

- **`hotfix/<name>`** — an urgent fix for a released version. Mechanically a feature branch (off `develop`, merged into `develop` by PR); the name signals urgency, and a release (`develop` → `main`, new patch version + tag) follows immediately. PRs into `main` accept only `develop`, so there is no direct fix-to-main path; keep `develop` releasable.

## Deploy branches (auto-generated — never commit by hand)

CI force-pushes the built landing site to these branches; they contain only build
output (no source), and each deploy replaces the branch wholesale:

- **`pages/landing`** — the **production** landing build, published from `main`.
- **`pages/landing-dev`** — the **staging** landing build, published from
  `develop`, to preview changes before they reach production. It keeps its
  `-dev` name: it is what serves the staging site, and the name describes the
  environment rather than the branch the build came from.

A ruleset blocks deleting these branches; not pushing to them by hand is a
convention (see [Branch protections](#branch-protections)).

See [deployment.md](deployment.md) for how these are built and served.

## Branch protections

GitHub **rulesets** enforce the flow above, with no admin bypass; the rules
bind everyone, including the repo owner:

- **`protect-main`**: `main` only changes through merged pull requests
  (0 required approvals, solo maintainer); force pushes and deletion are
  blocked. The required **release source branch** status check
  ([`gitflow.yml`](../.github/workflows/gitflow.yml)) fails any PR whose head
  branch is not `develop`: releasing from `develop` is the only way into `main`.
- **`protect-develop`**: the same for `develop`, plus the required **gitflow branch
  name** status check ([`gitflow.yml`](../.github/workflows/gitflow.yml)),
  which fails any PR whose head branch is not `feature/*`, `hotfix/*`,
  `release/*`, or
  `main` (a back-merge). GitHub cannot natively restrict a PR's source branch,
  so the check is what makes the gitflow naming binding.
- **`protect-pages`**: the `pages/**` build branches cannot be deleted.
  Nothing stronger is enforced: CI publishes them with `GITHUB_TOKEN`
  force-pushes, and on a personal repo that actor cannot be granted a ruleset
  bypass, so any rule blocking human pushes would block the deploys too. A
  stray manual push is overwritten by the next deploy anyway.

## Releasing

When `develop` reaches a state worth shipping, open a pull request from `develop` into `main`, merge it, and tag the merge commit with the version number. `main` therefore only ever moves forward one version at a time.

Pushing the tag triggers [`release.yml`](../.github/workflows/release.yml), which builds the CLI binaries (macOS arm64 and Intel tarballs) and the desktop app (one universal macOS `.dmg`, `.deb`/`.rpm`/`.AppImage` builds for x86_64 Linux, and an `.msi` plus NSIS setup `.exe` for x64 Windows), all unsigned for now, and publishes a GitHub release with them, their sha256 checksums, and auto-generated notes. The build also runs the Rust test suite once on the macOS runner — the one shipped OS regular CI never exercises, since CI is Linux-only — so a macOS-only test failure blocks the release. Only exact `vX.Y.Z` tags trigger it, and a guard job refuses to publish when the tagged commit is not on `main` (a tag placed on `develop` by mistake never becomes a release) or when the tag does not match the versions in `apps/cli/Cargo.toml` and `apps/desktop/src-tauri/tauri.conf.json` — bump **both** before tagging, or the CLI would report the wrong `--version` and the desktop bundles (`.dmg`, `.deb`, `.rpm`, `.AppImage`, `.msi`, setup `.exe`) would carry the wrong version in their names. The `workflow_dispatch` trigger dry-runs the builds without publishing anything, even if pointed at a tag.

## Commits

We favor **small commits with few changes** over large ones:

- One logical change per commit — a commit should be explainable in one sentence.
- Prefer several small commits over one big commit, even within a single task.
- Red commits are fine on `feature/*`/`hotfix/*` branches while work is in progress (say so in the message); whatever merges into `develop` must pass `cargo test`, so every commit on `develop` and `main` is green.
- Message format: a short imperative summary line, optionally prefixed by area (`core:`, `docs:`, `chore:`), with a body only when the *why* is not obvious from the diff.
