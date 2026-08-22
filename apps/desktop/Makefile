.DEFAULT_GOAL := build
.PHONY: install test test-rust dev build bundle bundle-linux bundle-windows compile clean

# Install JS deps only when the lockfile changed.
node_modules: package-lock.json
	npm ci
	@touch node_modules

# tauri::generate_context!() embeds this bundle at compile time, so the Rust
# crate does not compile without it. `npm run tauri build` produces it through
# beforeBuildCommand; a plain `cargo test` has no such hook, hence this target.
dist: node_modules
	npm run build

install: node_modules ## Install JS dependencies

test: node_modules ## Run the Vitest suite (Tauri IPC mocked)
	npm run test

test-rust: dist ## Run the src-tauri Rust integration tests
	cd src-tauri && cargo test

dev: node_modules ## Run the Tauri app in dev mode (hot reload)
	npm run tauri dev

build: node_modules ## Build the Tauri bundles for this OS
	npm run tauri build

bundle: node_modules ## Build the distributable macOS app: one universal .dmg
	npm run tauri -- build --target universal-apple-darwin --bundles dmg

# APPIMAGE_EXTRACT_AND_RUN: the AppImage tooling Tauri downloads (linuxdeploy,
# appimagetool) ships as AppImages itself, and those self-mount through FUSE 2,
# which Ubuntu 24.04 and the CI runners no longer provide. Extracting them
# instead works with or without FUSE.
bundle-linux: node_modules ## Build the distributable Linux packages: .deb, .rpm and .AppImage (host arch)
	APPIMAGE_EXTRACT_AND_RUN=1 npm run tauri -- build --bundles deb,rpm,appimage

bundle-windows: node_modules ## Build the distributable Windows installers: .msi and NSIS setup .exe
	npm run tauri -- build --bundles msi,nsis

compile: node_modules ## Compile the app (frontend + Rust) without bundling installers
	npm run tauri -- build --no-bundle

clean: ## Remove JS and Tauri build artifacts
	rm -rf node_modules dist .output src-tauri/target src-tauri/gen
