# Desktop app

`apps/desktop` is the Collapse desktop app: a **Tauri v2** shell with a Vue 3
frontend and a small Rust backend that calls `collapse-core` directly. It
compresses files and folders (7z / ZIP / tar) and extracts archives, in a calm
interface using the cervantic palette (warm cream + terracotta, monospace).

It runs on **macOS, Windows and Linux** from one codebase.

## Layout

```
apps/desktop/
  src/                Vue 3 frontend (App.vue is the whole UI)
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
Distribute `.deb` / `.AppImage` / `.rpm`; Flathub is an optional later channel.

## Notes

- The window uses the macOS overlay title bar (`titleBarStyle: "Overlay"`,
  `hiddenTitle`); on Windows/Linux Tauri falls back to standard decorations.
- Continuous integration for the desktop build (installing the Tauri system deps,
  building per-OS) is tracked separately — it is intentionally not part of the
  lean `cargo test` pipeline.
