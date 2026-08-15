.DEFAULT_GOAL := build
.PHONY: build release test run fmt lint clean

build: ## Build collapse-cli (binary: collapse)
	cargo build -p collapse-cli

release: ## Release build for a Rust target triple (TARGET=aarch64-apple-darwin)
ifndef TARGET
	$(error TARGET is required, e.g. make release TARGET=aarch64-apple-darwin)
endif
	cargo build -p collapse-cli --release --locked --target $(TARGET)

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
