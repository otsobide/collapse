.DEFAULT_GOAL := build
.PHONY: install test dev build bundle compile clean

# Install JS deps only when the lockfile changed.
node_modules: package-lock.json
	npm ci
	@touch node_modules

install: node_modules ## Install JS dependencies

test: node_modules ## Run the Vitest suite (Tauri IPC mocked)
	npm run test

dev: node_modules ## Run the Tauri app in dev mode (hot reload)
	npm run tauri dev

build: node_modules ## Build the Tauri bundles for this OS
	npm run tauri build

bundle: node_modules ## Build the distributable macOS app: one universal .dmg
	npm run tauri -- build --target universal-apple-darwin --bundles dmg

compile: node_modules ## Compile the app (frontend + Rust) without bundling installers
	npm run tauri -- build --no-bundle

clean: ## Remove JS and Tauri build artifacts
	rm -rf node_modules dist .output src-tauri/target src-tauri/gen
