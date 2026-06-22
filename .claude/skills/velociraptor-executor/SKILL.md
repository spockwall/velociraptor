---
name: velociraptor-executor
description: Velociraptor executor crate — order REST gateway. Ports, run command, REST action mapping per exchange, Redis kill-switch / dead-man-switch / audit stream, pre-flight risk gates, tests. Use whenever working on order placement, risk, or executor operations.
---

# Velociraptor — Executor (Order REST Gateway)

Order-side counterpart to `orderbook_server`. Accepts msgpack `OrderRequest` frames on a ZMQ ROUTER socket and dispatches to per-exchange REST clients (currently **Kalshi** and **Polymarket CLOB v2**). The trading engine connects a `DEALER` socket and gets typed `OrderResponse` replies.

For the wire types see `velociraptor-zmq-protocol`.

## Ports

| Service | Port | Protocol | Notes |
|---|---|---|---|
| ROUTER (orders) | 5557 | ZMQ | DEALER → ROUTER, msgpack `OrderRequest` |
| Metrics | 5558 | HTTP | Prometheus text format on `/metrics` |

## Run

```bash
cargo run --bin executor --release -- \
  --config configs/dev/config.yaml \
  --credentials credentials/polymarket.yaml \
  --kalshi-credentials credentials/kalshi.yaml
```

`--credentials` holds the `polymarket:` section; **Kalshi creds are a separate
file** (`--kalshi-credentials`, default `credentials/kalshi.yaml`) because they
live apart from Polymarket. The Kalshi REST client is built only when that file
has a `kalshi:` section — absent → logged and skipped, startup still succeeds.
Both credential files are checked for `chmod 600` at startup; pass
`--skip-chmod-check` in dev.

## Action → REST mapping

Dispatch is exchange-generic: `Executor::dispatch` looks up
`clients.get(&req.exchange)` and calls the `RestOrderClient` trait. Send
`exchange: "kalshi"` (lowercase, `ExchangeName` is `rename_all="lowercase"`) to
hit the Kalshi client.

| `OrderAction` | Kalshi (`external-api` v2) | Polymarket v2 |
|---|---|---|
| `Place` | `POST /portfolio/events/orders` (limit only) | `POST /order` (EIP-712 signed) |
| `PlaceBatch` | `POST /portfolio/events/orders/batched` | iterate `place` |
| `Update` | `GET` order → `POST /portfolio/events/orders/{id}/amend` | Internal — emulate via cancel+place |
| `Cancel` | `DELETE /portfolio/orders/{id}` (idempotent: 404 → Canceled) | SDK `cancel_order` |
| `CancelAll` | list `GET /portfolio/orders` → DELETE each | `DELETE /cancel-all` |
| `CancelMarket` | list → DELETE each row matching the ticker | `DELETE /cancel-market-orders` |
| `Heartbeat` | static ack (Kalshi REST has no heartbeat) | static ack |

Kalshi conventions (`executor/src/rest/kalshi.rs`):
- **symbol** = UPPERCASE Kalshi **market** ticker (e.g. `KXBTC15M-…-15` or
  `KXMENWORLDCUP-26-AR`), NOT the event ticker — a wrong/event ticker 404s
  `market_not_found`.
- **side** `Buy`→`bid`, `Sell`→`ask` (the orders-list reports `yes`/`no`, which
  amend remaps back to bid/ask).
- **px** in dollars, 4-dp string (`"0.5600"`); **qty** = contract count, 2-dp.
- `self_trade_prevention_type` is **required** (sent as `taker_at_cross`).
- **kind**: every order is sent as a limit on the wire (`price` + `time_in_force`,
  both required; no `type` field). `OrderKind::Limit` uses the caller's px;
  `OrderKind::Market` sends an aggressive limit at the worst-acceptable price
  (`0.99` buy / `0.01` sell — the in-range extremes; `1.00`/`0.00` are rejected
  `invalid_price`) with IOC/FOK, so it crosses and fills immediately.
- **Polymarket:** decimal token-id string.

## Redis control plane

| Key | Purpose |
|---|---|
| `executor:kill_switch` | `"1"` blocks all non-cancel actions globally |
| `executor:kill_switch:{exchange}` | Per-exchange tier (e.g. `executor:kill_switch:kalshi`) |
| `executor:cancel_all` | One-shot trigger — executor calls `cancel_all()` per client and `DEL`s the key |
| `executor:backend_heartbeat` | Backend writes unix-secs every 5s; stale >30s engages dead-man |
| `executor:log` | Append-only audit stream (XADD, MAXLEN ~100k default) |

Operator runbook:

```bash
redis-cli SET executor:kill_switch 1     # halt all non-cancel orders
redis-cli DEL executor:kill_switch       # resume
redis-cli SET executor:cancel_all 1      # flush every open order; auto-DEL once consumed
redis-cli XLEN executor:log
redis-cli XRANGE executor:log - + COUNT 5
```

## Audit trail (dual-sinked)

Every request and response is written to two places:

1. **Redis stream** `executor:log` (`XADD MAXLEN ~ <cap>`).
2. **Disk** `./data/executor/{date}.mpack` — length-prefixed msgpack frames, identical bytes to the stream payload.

Each entry carries `seq` and `prev_hash` (BLAKE3 over the prior entry's encoded bytes) — tampering between executor and backend is detectable. **Critical entries** (errors, kill-switch transitions, cancel-all triggers) are **fsynced immediately**; routine entries ride the buffer.

## Pre-flight risk gates

Run in order, fail-fast, for every `Place` / batch leg:

1. Notional cap per order
2. Token-bucket rate limit per exchange
3. Stale-quote guard (BBA age vs. `max_quote_age_secs`)
4. Price-deviation guard (vs. BBA mid in bps)
5. Open-order count cap per `(exchange, symbol)`
6. Aggregate notional cap per exchange

Failures return `OrderError::RiskRejected { rule, detail }` — the engine sees the typed error directly. Counters are in-process and reconciled against `get_orders()` periodically.

## Dead-man-switch

Above all risk checks. The engine sends `Heartbeat` at cadence shorter than `next_due_ms` (e.g. every 5s for a 15s window). If executor misses `2 × interval`:

1. Calls `cancel_all` on every configured exchange.
2. Flips `risk:kill_switch="true"` in Redis.

## Tests

```bash
cargo test -p executor                                  # unit + ZMQ round-trip
cargo test -p executor --features integration_tests     # live Kalshi demo / Polymarket testnet (creds required)
```
