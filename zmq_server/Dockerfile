# syntax=docker/dockerfile:1.6
# Pull the orderbook_server binary from the shared builder image. See
# `Dockerfile.builder` at the repo root.
ARG BUILDER_IMAGE=velociraptor/builder:latest
FROM ${BUILDER_IMAGE} AS bins

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends libzmq5 ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=bins /src/target/release/orderbook_server /usr/local/bin/orderbook_server
ENTRYPOINT ["orderbook_server"]
