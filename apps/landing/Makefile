.DEFAULT_GOAL := build
.PHONY: install dev build preview clean

# Install JS deps only when the lockfile changed.
node_modules: package-lock.json
	npm ci
	@touch node_modules

install: node_modules ## Install JS dependencies

dev: node_modules ## Run the Nuxt dev server (port 3000)
	npm run dev

build: node_modules ## Generate the static site into .output/public
	npm run generate

preview: node_modules ## Preview the built site
	npm run preview

clean: ## Remove build artifacts
	rm -rf node_modules .nuxt .output dist
