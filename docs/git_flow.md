# Git flow

This repository follows a **gitflow** branching model, kept deliberately simple.

## Source branches

Where humans commit code:

- **`main`** — released versions only. Every merge into `main` marks a version and gets a tag (`vX.Y.Z`). Changes arrive only through pull requests (see [Branch protections](#branch-protections)).
- **`dev`** — the integration branch where real day-to-day work happens. All work lands here first, always through a pull request from a gitflow branch; direct pushes are blocked.
- **`feature/<name>`** — branched off `dev` for each unit of work, merged back into `dev` via pull request when done. Since direct pushes to `dev` are blocked, even small changes travel on a short-lived feature branch.
- **`hotfix/<name>`** — branched off `main` when a released version needs an urgent fix; merged back (via pull requests) into both `main` (new patch version + tag) and `dev`.

## Deploy branches (auto-generated — never commit by hand)

CI force-pushes the built landing site to these branches; they contain only build
output (no source), and each deploy replaces the branch wholesale:

- **`pages/landing`** — the **production** landing build, published from `main`.
- **`pages/landing-dev`** — the **staging** landing build, published from `dev`, to preview changes before they reach production.

A ruleset blocks deleting these branches; not pushing to them by hand is a
convention (see [Branch protections](#branch-protections)).

See [deployment.md](deployment.md) for how these are built and served.

## Branch protections

GitHub **rulesets** enforce the flow above, with no admin bypass; the rules
bind everyone, including the repo owner:

- **`protect-main`**: `main` only changes through merged pull requests
  (0 required approvals, solo maintainer); force pushes and deletion are
  blocked.
- **`protect-dev`**: the same for `dev`, plus the required **gitflow branch
  name** status check ([`gitflow.yml`](../.github/workflows/gitflow.yml)),
  which fails any PR whose head branch is not `feature/*`, `hotfix/*`, or
  `main` (a back-merge). GitHub cannot natively restrict a PR's source branch,
  so the check is what makes the gitflow naming binding.
- **`protect-pages`**: the `pages/**` build branches cannot be deleted.
  Nothing stronger is enforced: CI publishes them with `GITHUB_TOKEN`
  force-pushes, and on a personal repo that actor cannot be granted a ruleset
  bypass, so any rule blocking human pushes would block the deploys too. A
  stray manual push is overwritten by the next deploy anyway.

## Releasing

When `dev` reaches a state worth shipping, open a pull request from `dev` into `main`, merge it, and tag the merge commit with the version number. `main` therefore only ever moves forward one version at a time.

## Commits

We favor **small commits with few changes** over large ones:

- One logical change per commit — a commit should be explainable in one sentence.
- Prefer several small commits over one big commit, even within a single task.
- Red commits are fine on `feature/*`/`hotfix/*` branches while work is in progress (say so in the message); whatever merges into `dev` must pass `cargo test`, so every commit on `dev` and `main` is green.
- Message format: a short imperative summary line, optionally prefixed by area (`core:`, `docs:`, `chore:`), with a body only when the *why* is not obvious from the diff.
