.DEFAULT_GOAL := build
.PHONY: build test fmt lint clean

build: ## Build collapse-remote
	cargo build -p collapse-remote

test: ## Run collapse-remote tests
	cargo test -p collapse-remote

fmt: ## Format collapse-remote
	cargo fmt -p collapse-remote

lint: ## Clippy collapse-remote
	cargo clippy -p collapse-remote --all-targets

clean: ## Remove collapse-remote build artifacts
	cargo clean -p collapse-remote
