.DEFAULT_GOAL := help

APPS := core remote cli server-backend server-frontend server-aio desktop landing

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
test: core/test remote/test cli/test server-backend/test server-frontend/test desktop/test desktop/test-rust ## Run every test suite

# desktop/test-rust is deliberately absent: the src-tauri crate does not compile
# without the frontend bundle, so its Rust suite needs the Node toolchain that
# this target exists to avoid.
.PHONY: test/rust
test/rust: core/test remote/test cli/test server-backend/test ## Run the Rust tests that need no Node toolchain

.PHONY: build
build: core/build remote/build cli/build server-backend/build ## Build the Rust crates (debug)

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
# Docker — collapse-server-backend in a container (see docker-compose.yml)
#
# "docker" is not in APPS, so these do not collide with the <app>/<target>
# pattern rules above. Override the port or the image name inline, e.g.
# `COLLAPSE_PORT=9000 make docker/up`.
# ---------------------------------------------------------------------------
COLLAPSE_PORT ?= 8000
COLLAPSE_WEB_PORT ?= 8080
IMAGE ?= collapse-server-backend:dev
WEB_IMAGE ?= collapse-server-frontend:dev
AIO_IMAGE ?= collapse-server-aio:dev
COMPOSE ?= docker compose

.PHONY: docker/build
docker/build: ## Build both images (backend and web frontend)
	docker build -f apps/server-backend/Dockerfile -t $(IMAGE) .
	docker build -f apps/server-frontend/Dockerfile -t $(WEB_IMAGE) .

.PHONY: docker/up
docker/up: ## Start the stack in the background and print its URLs
	COLLAPSE_PORT=$(COLLAPSE_PORT) COLLAPSE_WEB_PORT=$(COLLAPSE_WEB_PORT) $(COMPOSE) up -d --build
	@echo "web app at http://localhost:$(COLLAPSE_WEB_PORT)"
	@echo "API docs at http://localhost:$(COLLAPSE_PORT)/docs"

.PHONY: docker/aio
docker/aio: ## Start the all-in-one container (same ports, one image)
	COLLAPSE_PORT=$(COLLAPSE_PORT) COLLAPSE_WEB_PORT=$(COLLAPSE_WEB_PORT) $(COMPOSE) --profile aio up -d --build aio
	@echo "web app at http://localhost:$(COLLAPSE_WEB_PORT)"
	@echo "API docs at http://localhost:$(COLLAPSE_PORT)/docs"

.PHONY: docker/down
docker/down: ## Stop the stack and remove its containers
	$(COMPOSE) --profile aio down

.PHONY: docker/logs
docker/logs: ## Follow the container logs
	$(COMPOSE) logs -f

.PHONY: docker/shell
docker/shell: ## Open a shell inside the running backend container
	$(COMPOSE) exec backend /bin/bash

.PHONY: docker/run
docker/run: docker/build ## Run a throwaway container — ARGS="--max-upload-mb 50"
	docker run --rm -p $(COLLAPSE_PORT):8000 $(IMAGE) $(ARGS)

.PHONY: docker/smoke
docker/smoke: ## Build, start, drive a real compression through the published port, stop
	@apps/server-backend/smoke.sh $(COLLAPSE_PORT) $(IMAGE)

.PHONY: docker/clean
docker/clean: ## Remove the containers, the volume and the images
	-$(COMPOSE) --profile aio down -v
	-docker image rm $(IMAGE) $(WEB_IMAGE) $(AIO_IMAGE)

.PHONY: help
help: ## Show this help
	@echo "Collapse — make targets:"
	@grep -hE '^[a-zA-Z0-9_/-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'
	@echo "  \033[36m<app>/<t>\033[0m   run target <t> in an app — apps: $(APPS)"
