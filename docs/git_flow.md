# Git flow

This repository follows a **gitflow** branching model, kept deliberately simple.

## Source branches

Where humans commit code:

- **`main`** — released versions only. Every merge into `main` marks a version and gets a tag (`vX.Y.Z`). Nobody commits directly to `main`.
- **`dev`** — the integration branch where real day-to-day work happens. All work lands here first.
- **`feature/<name>`** — optional, branched off `dev` for larger or riskier units of work, merged back into `dev` when done. For small changes, committing directly on `dev` is fine.
- **`hotfix/<name>`** — branched off `main` when a released version needs an urgent fix; merged back into both `main` (new patch version + tag) and `dev`.

## Deploy branches (auto-generated — never commit by hand)

CI force-pushes the built landing site to these branches; they contain only build
output (no source), and each deploy replaces the branch wholesale:

- **`pages/landing`** — the **production** landing build, published from `main`.
- **`pages/landing-dev`** — the **staging** landing build, published from `dev`, to preview changes before they reach production.

See [deployment.md](deployment.md) for how these are built and served.

## Releasing

When `dev` reaches a state worth shipping, merge `dev` → `main` and tag the merge commit with the version number. `main` therefore only ever moves forward one version at a time.

## Commits

We favor **small commits with few changes** over large ones:

- One logical change per commit — a commit should be explainable in one sentence.
- Prefer several small commits over one big commit, even within a single task.
- Every commit should leave the repo in a working state (`cargo test` passes).
- Message format: a short imperative summary line, optionally prefixed by area (`core:`, `docs:`, `chore:`), with a body only when the *why* is not obvious from the diff.
