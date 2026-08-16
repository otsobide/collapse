.DEFAULT_GOAL := help

APPS := core remote cli api desktop landing

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
test: core/test remote/test cli/test api/test desktop/test ## Run every test suite

.PHONY: test/rust
test/rust: core/test remote/test cli/test api/test ## Run only the Rust tests

.PHONY: build
build: core/build remote/build cli/build api/build ## Build the Rust crates (debug)

.PHONY: fmt
fmt: ## Format all Rust code
	cargo fmt --all

.PHONY: lint
lint: ## Run clippy across the Rust workspace
	cargo clippy --workspace --all-targets

.PHONY: clean
clean: ## Remove Rust build artifacts (per-app: `make desktop/clean`)
	cargo clean

# ---------------------------------------------------------------------------
# Docker — collapse-api in a container (see docker-compose.yml)
#
# "docker" is not in APPS, so these do not collide with the <app>/<target>
# pattern rules above. Override the port or the image name inline, e.g.
# `COLLAPSE_PORT=9000 make docker/up`.
# ---------------------------------------------------------------------------
COLLAPSE_PORT ?= 8000
IMAGE ?= collapse-api:dev
COMPOSE ?= docker compose

.PHONY: docker/build
docker/build: ## Build the collapse-api image
	docker build -f apps/api/Dockerfile -t $(IMAGE) .

.PHONY: docker/up
docker/up: ## Start the API in the background and print its docs URL
	COLLAPSE_PORT=$(COLLAPSE_PORT) $(COMPOSE) up -d --build
	@echo "collapse-api is up — docs at http://localhost:$(COLLAPSE_PORT)/docs"

.PHONY: docker/down
docker/down: ## Stop the API and remove its container
	$(COMPOSE) down

.PHONY: docker/logs
docker/logs: ## Follow the API container logs
	$(COMPOSE) logs -f api

.PHONY: docker/shell
docker/shell: ## Open a shell inside the running API container
	$(COMPOSE) exec api /bin/bash

.PHONY: docker/run
docker/run: docker/build ## Run a throwaway container — ARGS="--max-upload-mb 50"
	docker run --rm -p $(COLLAPSE_PORT):8000 $(IMAGE) $(ARGS)

.PHONY: docker/smoke
docker/smoke: ## Build, start, drive a real compression through the published port, stop
	@apps/api/smoke.sh $(COLLAPSE_PORT) $(IMAGE)

.PHONY: docker/clean
docker/clean: ## Remove the API container, its volume and the image
	-$(COMPOSE) down -v
	-docker image rm $(IMAGE)

.PHONY: help
help: ## Show this help
	@echo "Collapse — make targets:"
	@grep -hE '^[a-zA-Z0-9_/-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'
	@echo "  \033[36m<app>/<t>\033[0m   run target <t> in an app — apps: $(APPS)"
