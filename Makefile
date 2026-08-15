.DEFAULT_GOAL := help

APPS := core cli desktop landing

# ---------------------------------------------------------------------------
# Per-app delegation — `make <app>/<target>` runs <target> in apps/<app>/Makefile
# e.g. `make core/test`, `make desktop/dev`, `make cli/run ARGS="compress x"`
# ---------------------------------------------------------------------------
define APP_DELEGATE
$(1)/%:
	$$(MAKE) -C apps/$(1) $$*
endef
$(foreach app,$(APPS),$(eval $(call APP_DELEGATE,$(app))))

# ---------------------------------------------------------------------------
# Global targets
# ---------------------------------------------------------------------------
.PHONY: test
test: core/test cli/test desktop/test ## Run every test suite

.PHONY: test/rust
test/rust: core/test cli/test ## Run only the Rust tests

.PHONY: build
build: core/build cli/build ## Build the Rust crates (debug)

.PHONY: fmt
fmt: ## Format all Rust code
	cargo fmt --all

.PHONY: lint
lint: ## Run clippy across the Rust workspace
	cargo clippy --workspace --all-targets

.PHONY: clean
clean: ## Remove Rust build artifacts (per-app: `make desktop/clean`)
	cargo clean

.PHONY: help
help: ## Show this help
	@echo "Collapse — make targets:"
	@grep -hE '^[a-zA-Z0-9_/-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'
	@echo "  \033[36m<app>/<t>\033[0m   run target <t> in an app — apps: $(APPS)"
