# Velociraptor Python trading engine

A long-running, three-step engine for Polymarket up/down windows (BTC + ETH 15m by default). Step 0 is a passive observer across Binance / Polymarket / Kalshi to verify the market-data path; steps 1–2 place and reconcile orders. The engine discovers active windows from the backend, streams orderbooks from the `zmq_server` market PUB, sends place/cancel through the `executor` order ROUTER, and reconciles fills + order_updates from the `zmq_server` user PUB.

## Architecture

```
                ┌──────────────────────────┐
backend HTTP ──▶│ MarketDiscovery          │  /api/polymarket/markets → MarketWindow(up, down)
                │                          │  /api/kalshi/markets     → KalshiWindow(ticker)
                └──────────────────────────┘
                          │ rediscover every 30s
                          ▼
zmq_server PUB ──▶ MarketFeed (SUB :5555)   topic = "{exchange}:{symbol}"
                  │       │                  (binance, binance_spot, polymarket, kalshi)
                  │       ▼
                  │   ┌──────────────────────┐
                  │   │ Observer  (observe)  │  status table per tick — no orders
                  │   └──────────────────────┘
                  ▼
                ┌──────────────────────────────┐
                │ Strategy (probe / one_shot / │  per active window, per side (YES/NO)
                │           fill_once)         │
                └──────────────────────────────┘
                          │ place / cancel
                          ▼
executor ROUTER (DEALER :5557) ─── OrderRouter
                          ▲
                          │ fills, order_updates
zmq_server user PUB ──▶ UserFeed (SUB ipc:///tmp/trading/ws_status.sock)
```

## Prerequisites

The engine talks to three already-running services. Bring them up first
(host-native or via docker — either works):

| Service          | Endpoint                                      | Source                          |
|------------------|-----------------------------------------------|---------------------------------|
| backend          | `http://127.0.0.1:3000`                       | `cargo run --bin backend`       |
| zmq_server       | `tcp://127.0.0.1:5555` (market PUB)           | `cargo run --bin orderbook_server` |
| zmq_server       | `ipc:///tmp/trading/ws_status.sock` (user PUB)| (same)                          |
| executor         | `tcp://127.0.0.1:5557` (order ROUTER)         | `cargo run -p executor`         |

Easiest path: `make up` (see repo root README).

Python deps:

```bash
pip install pyzmq msgpack requests
```

Run from the **repo root** so the package import resolves cleanly.

## Strategies

Selected with `--strategy <name>`. Four names:

| Strategy     | Places? | Fills? | Terminates? | Use for                                              |
|--------------|---------|--------|-------------|------------------------------------------------------|
| `observe`    | no      | no     | on Ctrl-C   | Multi-exchange orderbook watcher; verify market data |
| `probe`      | yes     | no     | on Ctrl-C   | Far-from-touch place + cancel; verify the order pipe |
| `one_shot`   | yes     | no     | auto        | Single place + cancel per side, then exits           |
| `fill_once`  | yes     | yes    | on Ctrl-C   | Cross the spread; fill once per side per window      |

Legacy `--step 0/1/2` still works (`0→observe`, `1→probe`, `2→fill_once`) but prints a deprecation warning.

### `observe` — orderbook only (no orders)

Verifies the market-data path end-to-end before any orders fly. Subscribes Binance, Binance Spot, Polymarket, and Kalshi via the `zmq_server` market PUB, dumps a status table every `--report-secs` seconds grouped by exchange (bid / ask / mid / age + seconds left in the current Polymarket/Kalshi window).

```bash
python -m scripts.trading_engine --strategy observe
```

No connection to the executor, no orders, no user-channel feed. Safe to run against a stack with no executor at all. Window-rotation events show up as `subscribe …/up` and `unsubscribe …/up (window rotated)` log lines.

Override tracked symbols / series with `--binance-symbols`, `--binance-spot-symbols`, `--base-slugs`, `--kalshi-series`.

### `probe` — place + cancel forever (no fills)

Each tick, on each side: place a $0.01 buy at the **absolute price floor**, then cancel it synchronously in the same tick. Two safety properties:

- **Price = $0.01 (the tick floor).** Below every standing bid → the matching engine cannot match against any resting sell → no partial fills.
- **Cancel is immediate.** The cancel REST call fires right after the place ack returns, no "wait for next tick" gap. Order's lifetime on the book is bounded by two REST round-trips (~hundreds of ms).

Mid outside `[--safe-mid-low, --safe-mid-high]` (default `[0.30, 0.70]`) still skips that side.

```bash
python -m scripts.trading_engine --strategy probe
```

Watch:

- Engine logs: `PLACE px=… oid=…` on each tick per side.
- `python -m scripts.trading_engine.io.user_feed` (separate shell): `order_update` with status=`new` after each place, status=`canceled` after re-quote.
- Frontend → Account → Orders: same events arrive within ~2s.

Ctrl-C: the engine cancels every order it placed before exiting.

### `one_shot` — place once, cancel once, exit

Per side: place a single $0.01 buy (price floor — cannot fill), cancel synchronously in the same tick, mark the side done. Once both sides have completed, the engine exits cleanly. Same safety properties as `probe` (floor price + immediate same-tick cancel), but terminates after one round-trip per side instead of looping.

Engine-managed equivalent of the old `poly_place_cancel.py` script, but picks up the current Polymarket window automatically.

```bash
python -m scripts.trading_engine --strategy one_shot
```

### `fill_once` — fill exactly once per side per window

Quotes ask + 1 tick (crosses) so the order takes immediately. `$10 / px` tokens per order. After a fill arrives on a side, that side is parked until the next window.

```bash
python -m scripts.trading_engine --strategy fill_once --order-notional-usd 10
```

Watch for `FILL px=… qty=…` in the engine logs and `user.polymarket.fill` on the user-feed listener / frontend → Account → Fills.

## Diagnostics

Two standalone CLI entrypoints inside the package for plumbing checks. Both are read-only / safe — they never place an order.

### Heartbeat — round-trip the executor ROUTER

```bash
python -m scripts.trading_engine.io.order_router           # 3 heartbeats
python -m scripts.trading_engine.io.order_router --count 10
```

Sends N `Heartbeat` `OrderRequest`s on the DEALER socket and prints each `OrderResponse`. Exits non-zero if any response is an `Err` or has a mismatched `req_id`.

**Use it when:**

- After `make up` / restarting the executor — confirm the order ROUTER is bound and the executor is alive before launching a strategy.
- When `--strategy probe` logs `place failed: …` — narrows down whether the executor is unreachable vs. Polymarket itself is rejecting the order.
- After bringing up a remote stack — paired with `--endpoint tcp://10.0.0.5:5557`, confirms reachability before pointing the strategy at it.

It does **not** verify the user-channel feed (use the user-feed listener below) or market-data feed (use `--strategy observe`).

### User-event listener — tail fills + order_updates

```bash
python -m scripts.trading_engine.io.user_feed
python -m scripts.trading_engine.io.user_feed --topic-prefix user.polymarket.fill
```

SUBs the `zmq_server` user PUB and pretty-prints every event. Useful as a side-by-side console while a strategy runs.

## Common flags

| Flag                     | Default                                  | Meaning                                                                       |
|--------------------------|------------------------------------------|-------------------------------------------------------------------------------|
| `--strategy`             | `probe`                                  | `observe` / `probe` / `one_shot` / `fill_once`                                |
| `--step`                 | _(deprecated)_                           | Legacy: `0→observe`, `1→probe`, `2→fill_once`. Prints a warning.              |
| `--base-slugs`           | `btc-updown-15m eth-updown-15m`          | Polymarket base slugs to track / trade                                        |
| `--kalshi-series`        | `KXBTC15M KXETH15M`                      | Kalshi series to track (observe only)                                         |
| `--binance-symbols`      | `btcusdt ethusdt`                        | Binance USDT-M futures symbols (observe only)                                 |
| `--binance-spot-symbols` | `btcusdt ethusdt`                        | Binance spot symbols (observe only)                                           |
| `--report-secs`          | `5.0`                                    | `observe`: how often to dump the status table                                 |
| `--order-notional-usd`   | `10.0`                                   | Per-order notional cap                                                        |
| `--safe-mid-low`         | `0.30`                                   | Skip placing on a side whose mid is below this                                |
| `--safe-mid-high`        | `0.70`                                   | Skip placing on a side whose mid is above this                                |
| `--tick-secs`            | `2.0`                                    | Strategy tick interval                                                        |
| `--rediscover-secs`      | `30.0`                                   | How often to refresh the active-window list from the backend                  |
| `--backend-url`          | `http://127.0.0.1:3000`                  | Backend HTTP host                                                             |
| `--market-pub`           | `tcp://127.0.0.1:5555`                   | `zmq_server` market PUB                                                       |
| `--market-router`        | `tcp://127.0.0.1:5556`                   | `zmq_server` control-plane ROUTER                                             |
| `--user-pub`             | `ipc:///tmp/trading/ws_status.sock`      | `zmq_server` user PUB                                                         |
| `--router-endpoint`      | `tcp://127.0.0.1:5557`                   | Executor order ROUTER                                                         |
| `--log-level`            | `info`                                   | `debug` / `info` / `warning` / `error`                                        |

## Recipes

**Just BTC 15m, debug logs:**
```bash
python -m scripts.trading_engine --strategy probe \
    --base-slugs btc-updown-15m \
    --log-level debug
```

**One-time place + cancel sanity check (then exits):**
```bash
python -m scripts.trading_engine --strategy one_shot
```

**Smaller order size ($2 each):**
```bash
python -m scripts.trading_engine --strategy fill_once --order-notional-usd 2
```

**Point at a remote stack (e.g. docker on another host):**
```bash
python -m scripts.trading_engine --strategy probe \
    --backend-url http://10.0.0.5:3000 \
    --market-pub tcp://10.0.0.5:5555 \
    --user-pub tcp://10.0.0.5:5559 \
    --router-endpoint tcp://10.0.0.5:5557
```
> Note: the default user PUB is an IPC socket. To reach it across hosts /
> containers, configure `zmq_server` to expose a TCP user-PUB endpoint and
> point `--user-pub` at it.

## Safety behaviours built in

- **Pinned-floor guard.** If a side's `best_bid` is at or below the tick
  floor ($0.01), the engine doesn't place on that side and cancels any
  resting order it had there. Configurable per-instance via `pin_px=` on
  `WindowStrategy`.
- **Idempotent ticks.** If the live quote already matches target px+qty,
  the tick is a no-op (no API churn).
- **Soft cancels.** `NotFound` cancels are treated as success (order
  already terminated).
- **Graceful shutdown.** SIGINT/SIGTERM cancels every known order before
  exiting; window expiry triggers `cancel_all` for the expiring window.
- **Window rotation is automatic.** Re-runs `discover()` every
  `--rediscover-secs`, adds strategies for new windows, drops + cancels
  for expiring ones.

## Module layout

```
scripts/trading_engine/
├── __init__.py             # exports the public API
├── __main__.py             # python -m scripts.trading_engine
├── app.py                  # Engine class + main()
├── cli.py                  # parse_args()
│
├── io/                     # transport / external I/O
│   ├── executor_client.py  # low-level ZMQ DEALER + msgpack helpers
│   ├── order_router.py     # strategy-friendly wrapper over ExecutorClient
│   └── user_feed.py        # ZMQ SUB on user PUB → callback
│
├── market/                 # market-data inputs
│   ├── discovery.py        # HTTP poll of /api/polymarket/markets + /api/kalshi/markets
│   └── feed.py             # ZMQ SUB on market PUB; {(exchange,symbol) → Quote} map
│
└── trading/                # active logic
    ├── observer.py         # passive multi-exchange watcher with rotation logging
    └── strategies/         # one file per strategy + registry / factory
        ├── __init__.py     # make_strategy(name, ...) + available_strategies()
        ├── base.py         # Strategy abstract base, SideState, shared helpers
        ├── probe.py        # ProbeStrategy
        ├── fill_once.py    # FillOnceStrategy
        └── one_shot.py     # OneShotStrategy
```

Every executor-facing tool now lives inside this package. The standalone scripts that used to sit at `scripts/poly_*.py` have been folded in:

| Old standalone script         | Replaced by                                                            |
|-------------------------------|------------------------------------------------------------------------|
| `scripts/poly_heartbeat.py`    | `python -m scripts.trading_engine.io.order_router`                     |
| `scripts/poly_user_listener.py`| `python -m scripts.trading_engine.io.user_feed`                        |
| `scripts/poly_place_cancel.py` | `python -m scripts.trading_engine --strategy one_shot`                 |
| `scripts/executor_client.py`   | `from scripts.trading_engine.io import ExecutorClient, place, cancel`  |

### Adding a strategy

1. Create `trading/strategies/<name>.py` with a class subclassing `Strategy` from `.base`.
2. Set `name = "<name>"` on the class.
3. Implement `_target_px(quote)`. Optionally override `_extra_skip_reason` or `_after_place` for terminal behavior.
4. Register the class in `trading/strategies/__init__.py`'s `_REGISTRY`.

`--strategy <name>` will then pick it up; `available_strategies()` reflects the registry.

## Known limitations

- Every strategy currently quotes **buy** on both YES and NO. To test
  the sell path, change `side="buy"` in `strategies/base.py::_tick_one_side`
  or expose it as a flag.
- No startup reconciliation against `GET /api/orders` — orders left from
  a previous engine run aren't tracked. Fire the executor's kill switch
  to clear them: `docker compose exec redis redis-cli SET executor:cancel_all 1`.
- Geoblock applies: the executor process must egress from a Polymarket-
  allowed region. If running natively, your host VPN covers it; if
  running in docker, the container is on the bridge network and needs a
  VPN sidecar (see repo root README troubleshooting).
