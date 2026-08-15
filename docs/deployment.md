# Deployment

This covers how the **landing page** (`apps/landing`, a Nuxt static site) is
built and published. Distribution of the apps themselves — the CLI tarballs and
the desktop `.dmg` that `release.yml` attaches to each GitHub release — is a
separate topic: see [git_flow.md](git_flow.md#releasing) for the release
pipeline and [desktop.md](desktop.md) for the desktop bundle specifics.

## Two environments

The landing is published to a **build branch** that holds only the compiled site
(no source). Which branch depends on where the change landed:

| Source branch | Deploys to           | Environment | Purpose                                   |
|---------------|----------------------|-------------|-------------------------------------------|
| `dev`         | `pages/landing-dev`  | staging     | preview changes before they reach `main`  |
| `main`        | `pages/landing`      | production  | the live site                             |

So the normal flow is: iterate on `dev` → check the result on `pages/landing-dev`
→ merge `dev` into `main` (a release) → production updates on `pages/landing`.

```
commit to dev  ──▶  CI build  ──▶  pages/landing-dev   (staging)
merge to main  ──▶  CI build  ──▶  pages/landing       (production)
```

## How it works

The workflow is [`.github/workflows/deploy-landing.yml`](../.github/workflows/deploy-landing.yml).
On every push to `main` or `dev` (and via manual **Run workflow** / `workflow_dispatch`):

1. **Build** — `make landing/build`, which runs `npm ci` then `nuxt generate`,
   producing the static site in `apps/landing/.output/public`.
2. **Publish** — the contents of that directory (plus a `.nojekyll` file, so a
   static host serves Nuxt's `_nuxt/` asset dir) are **force-pushed** to the
   target branch as a single fresh commit. The target is chosen from the source
   branch: `main` → `pages/landing`, anything else → `pages/landing-dev`.

The deploy commit is authored as the repo owner (via the account's GitHub noreply
email), not a bot. Concurrency is keyed per source branch, so a `dev` deploy and a
`main` deploy never cancel each other.

Because each deploy force-pushes an orphan-style commit, the build branches never
accumulate history — they always reflect the latest build only. **Never commit to
them by hand.**

## Serving a build branch

The build branches are ready to serve as-is; point a host at one of them. For the
production site at a subdomain (e.g. `collapse.cervantic.com`):

1. **GitHub → Settings → Pages** → *Deploy from a branch* → branch `pages/landing`,
   folder `/ (root)`.
2. Set the **custom domain** to your subdomain, and add the matching **CNAME**
   record in DNS.

The site is built with `baseURL: /` (assets are referenced from the domain root),
so it must be served at a **domain/subdomain root** — not under a subpath like
`user.github.io/collapse/`, where `/_nuxt/…` would 404. Staging
(`pages/landing-dev`) can be served the same way from a separate subdomain (e.g.
`dev.collapse.cervantic.com`).

## What the page serves

The page is built around three platform download cards. Windows and Linux are
"Coming soon" placeholders; the **macOS card resolves its link at page load**:
`app.vue` fetches
`https://api.github.com/repos/otsobide/collapse/releases/latest` client-side
and links the first asset whose name ends in `.dmg` (labeling the button with
the release tag). With JavaScript disabled or the API unreachable, the button
falls back to the releases page.

That gives the live site a runtime dependency on the GitHub API, on the
hardcoded `otsobide/collapse` slug, and on every release containing exactly one
`.dmg` asset. Renaming the repo, making it private, or changing the desktop
bundle naming silently downgrades the button to the fallback link — nothing
fails in CI, because the landing has no test suite. Revisit `app.vue` if any of
those change.

## Local preview

```bash
make landing/dev       # dev server with hot reload (http://localhost:3000)
make landing/build     # produce the static site in apps/landing/.output/public
make landing/preview   # serve the built site locally
```
