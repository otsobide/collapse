.DEFAULT_GOAL := build
.PHONY: build run smoke clean

IMAGE ?= collapse-server-aio:dev

build: ## Build the all-in-one image
	cd $(CURDIR)/../.. && docker build -f apps/server-aio/Dockerfile -t $(IMAGE) .

run: ## Run it, publishing both ports
	docker run --rm -p 127.0.0.1:8080:8080 -p 127.0.0.1:8000:8000 $(IMAGE)

smoke: ## Build, start, drive a compression through both ports, stop
	@cd $(CURDIR)/../.. && apps/server-aio/smoke.sh $(IMAGE)

clean: ## Remove the image
	-docker image rm $(IMAGE)
