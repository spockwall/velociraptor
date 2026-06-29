# Convenience wrappers around docker compose. The Rust services share a
# `velociraptor/builder` image (Dockerfile.builder) — it must be built before
# the per-service runtime images, hence the two-step targets here.

.PHONY: help build builder rebuild frontend up down logs ps clean

# Show this help by default (bare `make`).
.DEFAULT_GOAL := help
help:
	@echo "velociraptor — docker compose wrappers"
	@echo ""
	@echo "Environment is selected with LABEL=<env> (NOT a --flag):"
	@echo "    make up LABEL=prod      production  (live CLOB, risk gate ON,  /data)"
	@echo "    make up LABEL=dev       dev         (testnet,   risk gate OFF, ./data)"
	@echo "    make up                 same as LABEL=dev  ← default mode is 'dev'"
	@echo ""
	@echo "    Current default: LABEL=$(LABEL)  DATA_DIR=$(DATA_DIR)"
	@echo ""
	@echo "Targets:"
	@echo "    make builder            build the shared Rust builder image"
	@echo "    make build              build builder + all runtime images (recreates frontend)"
	@echo "    make rebuild            clean --no-cache rebuild of everything"
	@echo "    make frontend           fast no-cache rebuild + redeploy of just the frontend"
	@echo "    make up   [LABEL=env]   start the stack detached"
	@echo "    make down               stop the stack"
	@echo "    make logs               follow logs (last 200 lines)"
	@echo "    make ps                 show service status"
	@echo "    make clean              stop + remove volumes and orphans"
	@echo ""
	@echo "Note: 'make up --mode prod' does NOT work — make only accepts VAR=value,"
	@echo "      so use 'make up LABEL=prod'."

# Build the shared Rust builder image, then the runtime images.
# The frontend is built with --no-cache: its Dockerfile's `COPY . . && npm run
# build` layer otherwise cache-hits across source edits, so a plain cached build
# serves a stale `dist/`. The Rust images keep the cache (they rebuild correctly
# off the freshly-built builder image).
#
# NOTE: building an image does NOT update a running container — `docker compose
# up -d` won't recreate a service whose image content changed under the same
# `:latest` tag. So after building we force-recreate the frontend; without this
# you keep seeing the old UI even though a fresh image exists.

export DOCKER_BUILDKIT=1
export COMPOSE_DOCKER_CLI_BUILD=1

# Environment label — selects which config + credentials each service loads:
#   configs/$(LABEL)/config.yaml  and  credentials/$(LABEL)/{polymarket,kalshi}.yaml
# Override per-invocation:  make up LABEL=prod   (defaults to dev).
# Exported so docker-compose's ${LABEL} substitution picks it up.
LABEL ?= dev
export LABEL

# Host data root, derived from LABEL: prod writes to /data, dev to ./data.
# Override explicitly with  make up LABEL=prod DATA_DIR=/mnt/whatever.
# Container paths stay /app/data regardless (compose maps $(DATA_DIR) -> /app/data).
ifeq ($(LABEL),prod)
DATA_DIR ?= /data
else
DATA_DIR ?= ./data
endif
export DATA_DIR

build:
	DOCKER_BUILDKIT=1 docker compose --profile build-only build builder
	DOCKER_BUILDKIT=1 docker compose build backend orderbook_server executor
	DOCKER_BUILDKIT=1 docker compose build --no-cache frontend
	docker compose up -d --force-recreate frontend

# Tight frontend-only loop: no-cache rebuild + redeploy just the frontend.
# Use this while iterating on the UI (seconds, not a full stack rebuild).
frontend:
	DOCKER_BUILDKIT=1 docker compose build --no-cache frontend
	docker compose up -d --force-recreate frontend

# Just the shared builder (use this after touching any Cargo.toml or src/).
builder:
	DOCKER_BUILDKIT=1 docker compose --profile build-only build builder

# Force a clean rebuild of everything.
rebuild:
	DOCKER_BUILDKIT=1 docker compose --profile build-only build --no-cache builder
	DOCKER_BUILDKIT=1 docker compose build --no-cache backend orderbook_server executor frontend

up:
	@test -f configs/$(LABEL)/config.yaml || { echo "ERROR: configs/$(LABEL)/config.yaml not found (LABEL=$(LABEL))"; exit 1; }
	@test -f credentials/$(LABEL)/polymarket.yaml || { echo "ERROR: credentials/$(LABEL)/polymarket.yaml not found (LABEL=$(LABEL))"; exit 1; }
	@echo "==> bringing up stack with LABEL=$(LABEL)  DATA_DIR=$(DATA_DIR)"
	docker compose up -d

down:
	docker compose down

logs:
	docker compose logs -f --tail=200

ps:
	docker compose ps

clean:
	docker compose down --volumes --remove-orphans
