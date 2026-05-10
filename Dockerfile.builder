# syntax=docker/dockerfile:1.6
#
# Shared builder image for all Rust services. Produces an image tagged
# `velociraptor/builder` whose `/src/target/release/` contains every workspace
# binary the runtime services need (orderbook_server, executor, backend).
#
# Each service Dockerfile then does `FROM velociraptor/builder AS bins` and
# `COPY --from=bins /src/target/release/<bin>` into a thin debian-slim
# runtime — one workspace compile, three runtime images.
#
# Layers:
#   chef:planner → cargo-chef recipe.json (depends only on Cargo.{toml,lock})
#   chef:cooker  → cargo chef cook --release (compiles all deps, cached
#                  whenever the recipe is unchanged — code edits don't bust this)
#   build        → COPY full source + cargo build --release of every bin
#   final        → keep target/release; this is the image other Dockerfiles
#                  COPY --from=...
#
# BuildKit cache mounts (--mount=type=cache) make the registry + git +
# target dirs survive between builds, so even rebuilds-after-edit are fast.

ARG RUST_VERSION=1
ARG DEBIAN_FRONTEND=noninteractive

# ── 1. Tooling base — shared by planner and cooker ──────────────────────────
FROM rust:${RUST_VERSION}-bookworm AS chef-base
WORKDIR /src
RUN apt-get update \
 && apt-get install -y --no-install-recommends libzmq3-dev pkg-config \
 && rm -rf /var/lib/apt/lists/* \
 && cargo install cargo-chef --locked --version ^0.1

# ── 2. Plan — cargo-chef recipe.json from the manifests ────────────────────
FROM chef-base AS planner
COPY Cargo.toml Cargo.lock ./
COPY libs/Cargo.toml       libs/Cargo.toml
COPY orderbook/Cargo.toml  orderbook/Cargo.toml
COPY recorder/Cargo.toml   recorder/Cargo.toml
COPY executor/Cargo.toml   executor/Cargo.toml
COPY backend/Cargo.toml    backend/Cargo.toml
COPY zmq_server/Cargo.toml zmq_server/Cargo.toml
# `cargo chef prepare` doesn't need source bodies, just enough of a layout
# that each manifest's package + targets resolve. We stub:
#   - `src/lib.rs` for every crate (the implicit lib target)
#   - every explicit `[[bin]]` path declared in any Cargo.toml
#   - every auto-discovered `src/bin/*.rs` already in the tree
#
# We DO NOT stub `src/main.rs` — that would inject a second implicit binary
# with the package name and collide with explicit `[[bin]]` entries (e.g.
# the executor crate would gain a duplicate `executor` binary).
RUN mkdir -p \
        libs/src orderbook/src recorder/src executor/src backend/src zmq_server/src \
        orderbook/src/bin executor/src/bin backend/src/bin zmq_server/src/bin \
 && for c in libs orderbook recorder executor backend zmq_server; do \
        : > $c/src/lib.rs; \
    done \
 && for b in \
        orderbook/src/bin/price_to_beat_backfill.rs \
        orderbook/src/bin/price_to_beat_fetcher.rs \
        orderbook/src/bin/asset_id_fetcher.rs \
        executor/src/bin/executor.rs \
        backend/src/bin/backend.rs \
        zmq_server/src/bin/orderbook_server.rs ; do \
        echo 'fn main() {}' > $b; \
    done
RUN cargo chef prepare --recipe-path recipe.json

# ── 3. Cook — compile dependencies once, cached across source edits ────────
FROM chef-base AS cooker
COPY --from=planner /src/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo chef cook --release --recipe-path recipe.json

# ── 4. Build — bring in real source, link the bins ─────────────────────────
FROM cooker AS build
COPY Cargo.toml Cargo.lock ./
COPY libs       libs
COPY orderbook  orderbook
COPY recorder   recorder
COPY executor   executor
COPY backend    backend
COPY zmq_server zmq_server
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release \
        --bin orderbook_server \
        --bin executor \
        --bin backend \
 && mkdir -p /out \
 && cp target/release/orderbook_server /out/ \
 && cp target/release/executor         /out/ \
 && cp target/release/backend          /out/

# ── 5. Final — minimal layer the service Dockerfiles COPY --from= ─────────
FROM debian:bookworm-slim AS final
WORKDIR /src/target/release
COPY --from=build /out/ ./
