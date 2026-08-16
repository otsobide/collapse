.DEFAULT_GOAL := build
.PHONY: build test run fmt lint clean

build: ## Build collapse-api
	cargo build -p collapse-api

test: ## Run collapse-api tests
	cargo test -p collapse-api

run: ## Run the API server — pass args via ARGS="--port 9000"
	cargo run -p collapse-api -- $(ARGS)

fmt: ## Format collapse-api
	cargo fmt -p collapse-api

lint: ## Clippy collapse-api
	cargo clippy -p collapse-api --all-targets

clean: ## Remove collapse-api build artifacts
	cargo clean -p collapse-api
