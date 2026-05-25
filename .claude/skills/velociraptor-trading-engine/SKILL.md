---
name: velociraptor-trading-engine
description: Python trading engine at `scripts/trading_engine/` — event-driven, single-strategy host that wraps the velociraptor ZMQ surface. Covers the Dispatcher / MarketState / Strategy lifecycle, per-callback throttling, rollover handling, and how to add a new strategy. Use when writing or modifying anything under `scripts/trading_engine/`.
---

# Velociraptor — Python Trading Engine

A long-running Python host that consumes velociraptor's market PUB + user PUB streams, runs **one** trading strategy per process, and routes orders through the executor. Lives at `scripts/trading_engine/`. Reference doc: `scripts/trading_engine/README.md`.

If you only need to subscribe to market data or send a single order from a one-off script, use `velociraptor-python-clients` instead — this skill is for the long-running engine.

## Core invariants

- **One engine = one strategy.** No in-process strategy dict; multi-market trading = multiple engine processes. Window strategies enforce exactly one `--base-slugs` at startup.
- **Fully event-driven.** No tick loop, no `on_timer`. Strategies declare `required_topics()` + register callbacks in `setup(dispatcher)`. Periodic logic piggy-backs on a quote callback for the relevant topic.
- **Single-threaded dispatcher.** All strategy callbacks (quote / trade / rollover / fill / order_update) run on the same thread → no locks, no reentrancy. Event order seen by the strategy = FIFO arrival order.
- **MarketState owns the live world.** Producers enqueue events; the dispatcher updates MarketState *before* firing callbacks. Strategies read state via methods (`market.quote`, `market.mid`, …) instead of poking the transport.
- **Per-callback throttling, default 2s.** `register_quote(..., min_interval_ms=N)` drops callback fires under the threshold. Default is `DEFAULT_MIN_INTERVAL_MS = 2000` ms — strategies that need every-frame cadence (probe / one_shot / fill_once / momentum's Polymarket leg) must pass `min_interval_ms=0` explicitly. State is updated on the dropped event, so the next non-throttled call sees fresh data.
- **Pre-start gating is server-side.** The new Polymarket / Kalshi window doesn't publish until its boundary, so the engine never sees a pre-start frame and doesn't need to filter for it.

## Architecture

```
MarketFeed (ZMQ SUB :5555) ─┐
                            ├─→ queue.Queue[Event]  →  Dispatcher (1 thread)
UserFeed   (ZMQ SUB :5559) ─┘                              │
                                                           │ 1. update MarketState
                                                           │ 2. fire registered callbacks
                                                           ▼
                                                       Strategy
                                                           │
                                                           ▼
                                                  OrderRouter → executor ROUTER :5557
```

## Module map

```
scripts/trading_engine/
├── app.py                          # Engine + main()
├── cli.py                          # parse_args(); enforces --base-slugs cardinality
│
├── market/
│   ├── feed.py                     # ZMQ SUB transport; producer cbs enqueue Events
│   └── state.py                    # MarketState — live cross-source quotes/trades/mid/history
│
├── typings/
│   ├── events.py                   # QuoteEvent / TradeEvent / RolloverEvent / FillEvent / OrderUpdateEvent / ShutdownEvent
│   ├── state.py                    # StrategyState + OrderLedger + OrderRecord
│   └── window.py                   # PolymarketWindow / KalshiWindow
│
├── io/                             # transport / external I/O — unchanged in the refactor
│   ├── executor_client.py / order_router.py / user_feed.py / event_log.py
│
└── trading/
    ├── dispatcher.py               # Dispatcher — single-threaded event router + throttle
    ├── helpers.py                  # Polymarket constants + clamp_px + qty_for_notional + safe_mid_guard
    └── strategies/
        ├── __init__.py             # _REGISTRY + make_strategy + available_strategies
        ├── base.py                 # Strategy abstract base (required_topics / setup / teardown)
        ├── observe.py             # ObserveStrategy (passive multi-exchange watcher)
        ├── probe.py                # ProbeStrategy
        ├── fill_once.py            # FillOnceStrategy
        ├── one_shot.py             # OneShotStrategy
        └── momentum.py             # MomentumStrategy
```

## Event types

Defined in `typings/events.py` — pure data, immutable dataclasses:

| Event              | Producer            | Fields                                          |
|--------------------|---------------------|-------------------------------------------------|
| `QuoteEvent`       | MarketFeed snapshot | `exchange`, `symbol`, `quote: Quote`            |
| `TradeEvent`       | MarketFeed trade    | `exchange`, `symbol`, `trade: Trade`            |
| `RolloverEvent`    | MarketFeed snapshot | `exchange`, `base_slug`, `full_slug`, `asset_id` (fires when `full_slug` changes) |
| `FillEvent`        | UserFeed            | `topic`, `ev: dict`                             |
| `OrderUpdateEvent` | UserFeed            | `topic`, `ev: dict`                             |
| `ShutdownEvent`    | signal handler      | sentinel — dispatcher returns when popped       |

**No `TimerEvent`.** Periodic logic must be triggered by a real data event.

## Dispatcher (the rate limiter)

`trading/dispatcher.py`. Single-threaded loop:

```python
def run(self) -> None:
    while True:
        ev = self._q.get()
        if isinstance(ev, ShutdownEvent):
            return
        self._dispatch(ev)
```

Throttling lives here — `_Registration` carries `min_interval_ns` + `last_fired_ns`; `_dispatch` updates `MarketState` unconditionally, then calls only the registrations whose throttle window has elapsed. Order/fill/rollover events bypass throttling (rare + critical).

Registration API:

```python
# Default min_interval_ms = DEFAULT_MIN_INTERVAL_MS (2000.0).
# Strategies that want every-frame cadence must pass min_interval_ms=0.
dispatcher.register_quote(exchange, symbol, cb, *, min_interval_ms=2000.0)
dispatcher.register_trade(exchange, symbol, cb, *, min_interval_ms=2000.0)
dispatcher.register_rollover(exchange, base_slug, cb)
dispatcher.register_order_update(cb)
dispatcher.register_fill(cb)
```

`stop()` enqueues a `ShutdownEvent` — safe from any thread. The dispatcher drains everything in front of it first (fills arriving on Ctrl-C are not lost).

## MarketState

`market/state.py`. The strategy's view of the world. Single-writer (dispatcher) / single-reader (strategy callbacks, same thread) → no locks.

Reads:
- `quote(ex, sym) -> Optional[Quote]`
- `trade(ex, sym) -> Optional[Trade]`
- `mid(ex, sym) -> Optional[float]` (Quote.mid, else `(bid+ask)/2`)
- `best_bid(ex, sym)` / `best_ask(ex, sym)`
- `recent_trades(ex, sym, n)` — bounded deque (default 64)
- `snapshot_all()` / `trades_all()`
- `current_asset_id(ex, base_slug)` — the live full_slug-side asset for a rolling topic

Writes (`_update_*`) are private — only the dispatcher calls them.

## Strategy lifecycle

`trading/strategies/base.py`. Three methods that subclasses fill in:

```python
class Strategy(abc.ABC):
    def __init__(self, *, market, router, state, window=None): ...

    def required_topics(self) -> list[tuple[str, str, str]]:
        # (exchange, symbol, kind="snapshot"|"trade") — engine subscribes these
        return []

    @abc.abstractmethod
    def setup(self, dispatcher: Dispatcher) -> None:
        # Register callbacks here.

    def teardown(self) -> None:
        # Cancel any live orders. Default no-op.
```

The engine builds the Strategy once, calls `required_topics()` to subscribe ZMQ topics (rolling topics skip the DEALER handshake; static exchanges use it), then calls `setup(dispatcher)`, then `dispatcher.run()` blocks the main thread.

`Strategy.label` returns the human-friendly tag (`window.full_slug or window.base_slug`, or `name` if no window). `_attribution()` returns `{"strategy": name, "label": label}` for the order event log.

## Windows + rollover

`PolymarketWindow` / `KalshiWindow` (in `typings/window.py`) carry per-window identity: `base_slug` (stable) + `full_slug` (per-window) + `up_asset_id` (per-window) + `window_start` / `window_end`.

Per-window strategies receive a partially-filled window at construction (`base_slug` set, rest None). The first incoming snapshot for that base_slug triggers a `RolloverEvent` that the strategy uses to fill in `full_slug` / `up_asset_id`. Subsequent rollovers (every 15 min for `*-updown-15m`) fire another `RolloverEvent`; the strategy resets its window-scoped state and trades the new `asset_id` without a restart.

**Always register a rollover handler in `setup()` if the strategy holds window-scoped state.**

## Strategy registry

`trading/strategies/__init__.py`:

```python
_REGISTRY: dict[str, Type[Strategy]] = {
    ObserveStrategy.name: ObserveStrategy,
    ProbeStrategy.name: ProbeStrategy,
    FillOnceStrategy.name: FillOnceStrategy,
    OneShotStrategy.name: OneShotStrategy,
    MomentumStrategy.name: MomentumStrategy,
}
```

CLI exposes the keys via `available_strategies()`. To add a new strategy: subclass `Strategy`, implement `required_topics()` + `setup()`, set `name = "<x>"`, register in `_REGISTRY`.

## Engine startup (`app.py`)

```python
def __init__(self, args):
    self.market = MarketState()
    self.queue = queue.Queue()
    self.market_feed = MarketFeed(args.market_pub, args.market_router,
        on_quote=self._enqueue_quote,
        on_trade=self._enqueue_trade,
        on_rollover=self._enqueue_rollover)
    self.user_feed = UserFeed(args.user_pub, on_event=self._enqueue_user_event)
    self.router = OrderRouter(args.router_endpoint, event_log=self.event_log)
    self.dispatcher = Dispatcher(self.queue, self.market)
    self.strategy = self._build_strategy()

def run(self) -> int:
    self._handle_signals()                  # SIGINT/SIGTERM → dispatcher.stop()
    self.market_feed.start()
    self._subscribe_required_topics()       # ZMQ subscribe per strategy.required_topics()
    self.strategy.setup(self.dispatcher)
    self.user_feed.start()
    with self.router:
        self.dispatcher.run()               # blocks main thread
    # teardown happens inside _run_dispatcher's finally
```

Observe bypasses the executor + user feed (it places no orders), so `--strategy observe` runs against a stack with no executor.

## CLI surface

`scripts/trading_engine/cli.py`. Key flags:

- `--strategy` (`observe`/`probe`/`one_shot`/`fill_once`/`momentum`).
- `--base-slugs` (window strategies → exactly one; observe → many).
- `--binance-symbols` / `--binance-spot-symbols` / `--kalshi-series` (observe only).
- `--order-notional-usd`, `--safe-mid-low`, `--safe-mid-high`.
- `--engine-log-dir` / `--no-engine-log`.

Removed in the event-driven refactor: `--tick-secs`, `--rediscover-secs`, `--report-secs`. Backend HTTP discovery is gone too — strategies subscribe rolling topics directly and detect rollover from the payload's `full_slug` (no `/api/polymarket/markets` polling).

## Common patterns

### Read cross-exchange data inside a callback

```python
def _on_poly_quote(self, q):
    binance_mid = self.market.mid("binance", "btcusdt")  # may be None if no quote yet
    if binance_mid is None:
        return
    # ... decide based on both
```

### Throttle a fast feed without losing freshness

```python
dispatcher.register_quote("binance", "btcusdt", self._sample,
                          min_interval_ms=50)
```
The callback fires at most every 50ms, but `self.market.mid("binance", "btcusdt")` always returns the latest quote because MarketState is updated unconditionally.

### Window-end exit without a timer

```python
def _on_poly_quote(self, q):
    now = time.time()
    if self.window.window_end and now > self.window.window_end - 120:
        self._force_close()
```

### Rollover reset

```python
def _on_rollover(self, full_slug, asset_id):
    self.window.full_slug = full_slug
    self.window.up_asset_id = asset_id
    self._live_oid = None
    self._filled_this_window = False
```

## Diagnostics (subcommands in the same package)

- `python -m scripts.trading_engine.io.order_router [--count N]` — heartbeat the executor ROUTER. Never places an order.
- `python -m scripts.trading_engine.io.user_feed [--topic-prefix user.polymarket.fill]` — tail fills + order_updates from the user PUB.

## Pitfalls

- **Never block in a callback.** All callbacks run on the dispatcher thread; a slow REST call stalls *every* event until it returns. `OrderRouter.place_limit` IS synchronous REST — but its latency budget is bounded (executor + Polymarket round-trip, ~hundreds of ms). If you need true async, push work onto a separate thread and have it enqueue a follow-up event.
- **Don't share state between strategy instances** — strategies are intentionally process-isolated. Cross-strategy coordination goes through Redis / executor.
- **Don't re-implement caches.** Use `MarketState.recent_trades(...)` instead of building a per-strategy deque (momentum keeps its own samples deque only for backward-compat — new strategies should read from state).
- **`required_topics()` is the only subscription surface.** If a topic isn't listed there, no callback you register against it will ever fire — the engine won't have subscribed the ZMQ topic.
- **Stale Docker image trap.** Engine-side bugs that look like "no data" often mean a stale `orderbook_server` image. Always check image age vs. source edit time before doubting Python diffs.

## Where to look next

- Wire formats / ZMQ topic shapes — `velociraptor-zmq-protocol`, `velociraptor-wire-formats`.
- Executor REST mapping / risk gates — `velociraptor-executor`.
- Rolling Polymarket/Kalshi key design + window mechanics — `velociraptor-polymarket`, `velociraptor-kalshi`.
- Redis keys the engine reads/writes via the executor — `velociraptor-backend-redis`.
- One-off pyzmq scripts (not the engine) — `velociraptor-python-clients`.
