# Convenience wrappers around docker compose. The Rust services share a
# `velociraptor/builder` image (Dockerfile.builder) — it must be built before
# the per-service runtime images, hence the two-step targets here.

.PHONY: build builder rebuild up down logs ps clean

# Build the shared Rust builder image, then the three runtime images.
build:
	DOCKER_BUILDKIT=1 docker compose --profile build-only build builder
	DOCKER_BUILDKIT=1 docker compose build backend orderbook_server executor frontend

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
