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

The page is one row per operating system: macOS (`.dmg`), Linux (`.deb`,
`.rpm`, AppImage) and Windows (still a "coming soon" placeholder). Every
download **resolves its link at page load**: `pages/index.vue` fetches
`https://api.github.com/repos/otsobide/collapse/releases/latest` client-side and
matches each button to the first release asset whose name ends in that
extension. A button whose asset is missing from the release renders as a
disabled "soon" chip instead, so the AppImage entry becomes a real download the
moment such an asset ships, with no page change. With JavaScript disabled or the
API unreachable, the available buttons fall back to the releases page.

## Languages

The site ships in **English and Spanish** via
[`@nuxtjs/i18n`](https://i18n.nuxtjs.org/), with the `prefix_except_default`
strategy: English is served at `/` and Spanish at `/es/`, and `nuxt generate`
prerenders both (they are also listed in `nitro.prerender.routes`, so a build
never depends on the crawler finding the switcher's links).

- **Copy** lives in `i18n/locales/{en,es}.json`, keyed by section. Components
  hold no literal copy.
- **Adding a language** is a locale file plus one entry in the `LOCALES` array
  at the top of `nuxt.config.ts`. That array is the single source: the i18n
  routing, the prerendered routes and the navbar menu all derive from it, and
  its `name` field (written in the language itself) is the menu's label.
- **The switcher** (`components/LanguageSwitcher.vue`) is a dropdown rather
  than inline links, so the header keeps its width as languages are added; the
  menu scrolls past roughly seven of them. Its links stay in the DOM while the
  menu is closed, so they remain crawlable.
- **SEO** comes from `useLocaleHead` in `app.vue`: per-locale `<title>`,
  description and `<html lang>`, plus `hreflang` alternates, `x-default` and a
  canonical. Those must be absolute URLs, so `i18n.baseUrl` defaults to the
  production origin (`https://collapse.cervantic.com`); override it with the
  `NUXT_PUBLIC_SITE_URL` environment variable when building for another origin.
  Staging builds keep the production canonical, which is what stops the preview
  from competing with the live site in search results.
- **First visit** to `/` follows the browser's language and redirects to `/es`
  for Spanish speakers; the choice is then remembered in the `i18n_redirected`
  cookie, so the navbar switcher always wins afterwards.

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
