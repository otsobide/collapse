.DEFAULT_GOAL := build
.PHONY: install test dev build preview clean

# Install JS deps only when the lockfile changed.
node_modules: package-lock.json
	npm ci
	@touch node_modules

install: node_modules ## Install JS dependencies

test: node_modules ## Run the Vitest suite
	npm run test

dev: node_modules ## Run the frontend against a local backend (port 5174)
	npm run dev

build: node_modules ## Build the static site into dist/
	npm run build

preview: node_modules ## Serve the built site locally
	npm run preview

clean: ## Remove JS build artifacts
	rm -rf node_modules dist
