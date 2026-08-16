.DEFAULT_GOAL := build
.PHONY: build test run fmt lint clean

build: ## Build collapse-server-backend
	cargo build -p collapse-server-backend

test: ## Run collapse-server-backend tests
	cargo test -p collapse-server-backend

run: ## Run the API server — pass args via ARGS="--port 9000"
	cargo run -p collapse-server-backend -- $(ARGS)

fmt: ## Format collapse-server-backend
	cargo fmt -p collapse-server-backend

lint: ## Clippy collapse-server-backend
	cargo clippy -p collapse-server-backend --all-targets

clean: ## Remove collapse-server-backend build artifacts
	cargo clean -p collapse-server-backend
