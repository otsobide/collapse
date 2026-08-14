.DEFAULT_GOAL := build
.PHONY: build test fmt lint clean

build: ## Build collapse-core
	cargo build -p collapse-core

test: ## Run collapse-core tests
	cargo test -p collapse-core

fmt: ## Format collapse-core
	cargo fmt -p collapse-core

lint: ## Clippy collapse-core
	cargo clippy -p collapse-core --all-targets

clean: ## Remove collapse-core build artifacts
	cargo clean -p collapse-core
