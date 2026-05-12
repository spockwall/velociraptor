# syntax=docker/dockerfile:1.6
# Pull the executor binary from the shared builder image. See
# `Dockerfile.builder` at the repo root and the `builder` service in
# docker-compose.yml — both must build before this one.
ARG BUILDER_IMAGE=velociraptor/builder:latest
FROM ${BUILDER_IMAGE} AS bins

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends libzmq5 ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=bins /src/target/release/executor /usr/local/bin/executor
ENTRYPOINT ["executor"]
