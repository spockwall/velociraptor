# syntax=docker/dockerfile:1.6
FROM rust:1-bookworm AS builder
WORKDIR /src
RUN apt-get update \
 && apt-get install -y --no-install-recommends libzmq3-dev pkg-config \
 && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY libs ./libs
COPY orderbook ./orderbook
COPY recorder ./recorder
COPY executor ./executor
COPY backend ./backend
COPY zmq_server ./zmq_server

RUN cargo build --release --bin orderbook_server

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends libzmq5 ca-certificates \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /src/target/release/orderbook_server /usr/local/bin/orderbook_server
ENTRYPOINT ["orderbook_server"]
