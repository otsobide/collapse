# Desktop app

`apps/desktop` is the Collapse desktop app: a **Tauri v2** shell with a Vue 3
frontend and a small Rust backend that calls `collapse-core` directly. It
compresses files and folders (7z / ZIP / tar) and extracts archives, in a calm
interface using the cervantic palette (warm cream + terracotta, monospace).

It runs on **macOS, Windows and Linux** from one codebase.

Compression can also be handed to a remote `collapse-server-backend` instance: the
compress options carry a **destination picker**, defaulting to this computer,
and the gear in the header opens a panel to add, test and remove servers. The
list is remembered between launches. The HTTP happens in Rust through
`collapse-remote`, not in the webview, which is what keeps the app's CSP at
`default-src 'self'` and needs no network capability. Extraction has no remote
mode, so the picker only appears when compressing.

## Layout

```
apps/desktop/
  src/                Vue 3 frontend (App.vue is the UI; paths.js holds the
                      path/format helpers, split out for unit testing)
  tests/              Vitest suite (App.test.js, paths.test.js — Tauri IPC mocked)
  src-tauri/          Rust backend
    src/lib.rs        Tauri commands: is_directory, compress_path, extract_archive
    tauri.conf.json   window, bundle, identifier (com.cervantic.collapse)
    capabilities/     window permissions (core + dialog)
    icons/            generated icon set (from app-icon.png)
    app-icon.png      1024² source icon — regenerate the set with `npx tauri icon`
    entitlements.plist  App Sandbox entitlements for the macOS App Store build
```

**The Rust crate is its own Cargo workspace** (note the empty `[workspace]` in
`src-tauri/Cargo.toml`), deliberately kept out of the root workspace so a plain
`cargo test` at the repo root (core + CLI) does not require the Tauri system
dependencies. Build the desktop app with its own toolchain, as below.

## Prerequisites

- Node.js 18+ and Rust (stable).
- Tauri v2 system dependencies for your OS — see
  <https://v2.tauri.app/start/prerequisites/>:
  - **macOS**: Xcode Command Line Tools.
  - **Windows**: Microsoft C++ Build Tools + the WebView2 runtime.
  - **Linux**: `webkit2gtk-4.1`, `libappindicator3`, `librsvg2`, `patchelf`, etc.

## Develop and build

```bash
cd apps/desktop
npm install
npm run tauri dev       # hot-reloading dev window
npm run tauri build     # produce installers/bundles for the current OS
npm run test            # frontend Vitest suite (or make desktop/test) — Node only
```

`tauri build` outputs to `apps/desktop/src-tauri/target/release/bundle/`:

- **macOS** → `.app` and `.dmg`
- **Windows** → `.msi` and an NSIS `.exe` installer
- **Linux** → `.deb`, `.AppImage`, `.rpm`

Regenerate the icon set after changing the artwork:

```bash
npx tauri icon src-tauri/app-icon.png
```

## Distribution

### macOS — direct download
Sign with a **Developer ID Application** certificate and notarize the app, then
ship the `.dmg`. Tauri wires signing/notarization through env vars
(`APPLE_CERTIFICATE`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`).

What ships **today**: every GitHub release (`release.yml`, on each `vX.Y.Z`
tag) includes an **unsigned universal `.dmg`** (arm64 + Intel), built by
`make desktop/bundle` (`tauri build --target universal-apple-darwin`, dmg
bundle only). Until signing/notarization lands, first launch requires
right-click → Open (or `xattr -d com.apple.quarantine`) to get past
Gatekeeper. The landing page's macOS button links this `.dmg`, resolved
client-side from the latest release — see
[deployment.md](deployment.md#what-the-page-serves).

### macOS — App Store
1. Join the Apple Developer Program; register the bundle id
   **`com.cervantic.collapse`** in App Store Connect.
2. Build sandboxed with the provided `src-tauri/entitlements.plist`
   (App Sandbox + user-selected file read/write — every path the app touches is
   chosen through a native dialog, which grants sandbox access).
3. Sign with a **Apple Distribution** cert + the App Store provisioning profile,
   then package with `productbuild` and upload via Transporter / `altool`.

The default `tauri build` is **not** sandboxed (it targets direct download); the
App Store build is a separate, entitlements-applied invocation.

### Windows
Distribute the `.msi` / NSIS installer (optionally code-signed with an
Authenticode certificate). The Microsoft Store is an optional later channel.

### Linux
What ships **today**: every GitHub release includes an x86_64 **`.deb`**,
**`.rpm`** and **`.AppImage`** (with sha256 checksums), built on the Ubuntu
runner by `make desktop/bundle-linux`
(`tauri build --bundles deb,rpm,appimage`). No Gatekeeper equivalent applies:
the packages install with `dpkg -i` / `rpm -i` as usual, and the AppImage is a
single portable file that runs anywhere once marked executable
(`chmod +x Collapse_X.Y.Z_amd64.AppImage`), with no install and no system
dependencies — it carries its own webkit.

Building the AppImage needs `APPIMAGE_EXTRACT_AND_RUN=1` (the make target sets
it): the tooling Tauri downloads for it ships as AppImages that self-mount
through FUSE 2, which Ubuntu 24.04 and the CI runners no longer provide.

arm64 packages and Flathub are optional later channels.

## Notes

- The window uses the macOS overlay title bar (`titleBarStyle: "Overlay"`,
  `hiddenTitle`); on Windows/Linux Tauri falls back to standard decorations.
- CI covers the desktop app in `test-and-build.yml`: a `vitest (desktop)` job
  runs the frontend suite (Node only, Tauri IPC mocked), and a `build (desktop)`
  job installs the Tauri Linux system deps and compiles the whole app via
  `make desktop/compile` (`tauri build --no-bundle`). That build job is the only
  thing that type-checks the `src-tauri` crate, including the remote path. The macOS universal
  `.dmg` is built by `release.yml` on each version tag. The `src-tauri` crate
  stays out of the root `cargo test` pipeline, which needs no Tauri deps.
