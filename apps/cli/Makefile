.DEFAULT_GOAL := build
.PHONY: build test run fmt lint clean

build: ## Build collapse-cli (binary: collapse)
	cargo build -p collapse-cli

test: ## Run collapse-cli tests
	cargo test -p collapse-cli

run: ## Run the CLI — pass args via ARGS="compress notes.txt -f 7z"
	cargo run -p collapse-cli -- $(ARGS)

fmt: ## Format collapse-cli
	cargo fmt -p collapse-cli

lint: ## Clippy collapse-cli
	cargo clippy -p collapse-cli --all-targets

clean: ## Remove collapse-cli build artifacts
	cargo clean -p collapse-cli
