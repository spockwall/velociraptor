# Convenience wrappers around docker compose. The Rust services share a
# `velociraptor/builder` image (Dockerfile.builder) — it must be built before
# the per-service runtime images, hence the two-step targets here.

.PHONY: build builder rebuild frontend up down logs ps clean

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
	docker compose up -d

down:
	docker compose down

logs:
	docker compose logs -f --tail=200

ps:
	docker compose ps

clean:
	docker compose down --volumes --remove-orphans
