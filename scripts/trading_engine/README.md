# Velociraptor Python trading engine

A long-running, **event-driven**, **single-strategy** engine for Polymarket up/down windows (and an `observe` mode that watches Binance + Binance Spot + Polymarket + Kalshi side-by-side without placing orders). The engine streams orderbooks from the `zmq_server` market PUB, sends place/cancel through the `executor` order ROUTER, and reconciles fills + order_updates from the `zmq_server` user PUB.

> **One engine = one strategy.** Trading multiple markets is expressed by launching multiple engine processes with different `--strategy` / `--base-slugs`.

## Architecture

```
┌──────────────────┐   ┌──────────────────┐
│ MarketFeed       │   │ UserFeed         │
│  ZMQ SUB :5555   │   │  ZMQ SUB :5559   │
└────────┬─────────┘   └────────┬─────────┘
         │ enqueue              │ enqueue
         ▼                      ▼
        ┌─────────────────────────────────────────┐
        │  queue.Queue[Event]                     │
        └────────────────────┬────────────────────┘
                             │ get()
                             ▼
        ┌─────────────────────────────────────────┐
        │  Dispatcher  (single thread)            │
        │   1. update MarketState                 │
        │   2. fire registered callbacks          │
        │      (subject to per-callback throttle) │
        └────────────────────┬────────────────────┘
                             │ method calls
                             ▼
        ┌─────────────────────────────────────────┐
        │  Strategy  (single instance)            │
        │   - required_topics() declares data deps│
        │   - setup(dispatcher) registers cbs     │
        │   - reads MarketState in callbacks      │
        │   - places via OrderRouter (REST)       │
        └─────────────────────────────────────────┘
                             │
                             ▼
        executor ROUTER (DEALER :5557)
```

There is no tick loop. Strategies react to events (quote / trade / rollover / fill / order_update). Periodic logic is expressed by piggy-backing on a quote callback for the relevant topic.

## Design — why it looks like this

The engine is the product of three deliberate trade-offs. Each is small in isolation; together they make the strategy code dramatically simpler than the earlier polling-based design.

### 1. One engine = one strategy (instead of in-process strategy fan-out)

Older versions held `dict[base_slug, Strategy]` and looped a tick over every entry. That meant the engine had to fan events out to the right strategy, support per-strategy enable/disable, and reason about cross-strategy contention. None of those exist now:

- The engine builds **exactly one** Strategy at startup. Every event goes to that one strategy. There is no routing layer between the dispatcher and the strategy.
- Trading two markets = two engine processes. Each gets its own Python, its own ZMQ context, its own MarketState, its own log directory.
- Window strategies (`probe` / `fill_once` / `one_shot` / `momentum`) enforce exactly one `--base-slugs` at startup; the engine errors out with a clear message if you pass more.
- Observe is the deliberate exception. It registers callbacks for an arbitrary number of (exchange, symbol) pairs but never trades — so the "one strategy" contract is preserved even when it touches many topics.

What you give up: cross-market coordination inside one Python process. If strategy A's fill should influence strategy B, they coordinate via the executor's Redis state, not shared memory. For the strategies in this repo, none of them need that.

### 2. Fully event-driven (instead of a tick loop)

There is no `tick()` and no `--tick-secs`. The reason: a tick-based loop's reaction latency floor equals the tick interval. A Binance trade arriving 1ms after the last tick is invisible to the strategy until the next tick fires. For momentum-style strategies, that's the dominant source of stale-quote risk.

The engine instead exposes five event kinds (see `typings/events.py`):

| Event              | Producer            | Throttled? | Notes                                                    |
|--------------------|---------------------|------------|----------------------------------------------------------|
| `QuoteEvent`       | MarketFeed snapshot | yes        | Strategies pick `min_interval_ms` per registration.      |
| `TradeEvent`       | MarketFeed trade    | yes        | Same throttle mechanism.                                 |
| `RolloverEvent`    | MarketFeed snapshot | no         | Fires when `full_slug` changes. Rare → no throttle.      |
| `FillEvent`        | UserFeed            | no         | Action-critical.                                         |
| `OrderUpdateEvent` | UserFeed            | no         | Action-critical.                                         |
| `ShutdownEvent`    | signal handler      | n/a        | Sentinel; dispatcher returns when it pops one.           |

**There is intentionally no `TimerEvent`.** Periodic logic that used to live in the tick loop is now expressed by piggy-backing on a real data event. For example, "force-close any open position in the last 120s of the window" runs inside the Polymarket quote callback in `momentum.py` — `time.time() vs self.window.window_end - 120`. The engine never wakes the strategy just to check the clock; if there's no data event, there's nothing useful the strategy could do anyway (it has no fresh quote to act on).

This decision came up explicitly during design and was rejected: "*don't use on_timer, it is confusing since we are event driven*."

### 3. Single-threaded Dispatcher (instead of per-strategy locks)

The dispatcher runs on the main thread. Producers (MarketFeed's SUB thread, UserFeed's SUB thread) only enqueue events; they never touch strategy or MarketState. The strategy itself is single-threaded by construction:

- No locks anywhere in the strategy code.
- No reentrancy — `on_fill` cannot interrupt `on_quote` mid-execution.
- Event order seen by the strategy = FIFO arrival order on the queue.
- MarketState is a single-writer/single-reader struct on the same thread.

The cost: callbacks must not block. A long-running REST call inside `on_quote` stalls every subsequent event for every callback the strategy registered. For this engine the tolerance is "hundreds of milliseconds" — `OrderRouter.place_limit` is synchronous but bounded by the executor + Polymarket round-trip. If a future strategy genuinely needs an asynchronous side channel, the pattern is: spawn a worker thread, have it post a follow-up event back onto the dispatcher queue when it's done. The strategy then reacts to that event on the dispatcher thread.

### 4. Per-callback throttling with always-fresh MarketState

When a Binance feed bursts at 200 quotes/sec, the strategy usually doesn't want to react 200×/sec — but it does want the *latest* mid the next time it does react. The dispatcher implements both halves.

The default throttle for `register_quote(...)` / `register_trade(...)` is **2000 ms** (`DEFAULT_MIN_INTERVAL_MS` in `dispatcher.py`). Strategies that need to fire on every frame must opt out explicitly with `min_interval_ms=0` — this matches what `probe`, `one_shot`, `fill_once`, and `momentum`'s Polymarket leg do today, since each places or amends orders on every quote. The default exists to protect new strategies that don't think about cadence yet (and the observe, which sets its own ~5s throttle on top).

```python
def _dispatch(self, ev):
    if isinstance(ev, QuoteEvent):
        self._market._update_quote(ev.exchange, ev.symbol, ev.quote)   # always
        self._fire(self._quote_regs.get((ev.exchange, ev.symbol)),     # subject to throttle
                   now_ns, ev.quote)
```

`MarketState` is updated unconditionally before the throttle check. A dropped (throttled-out) callback therefore loses nothing: the next time it does fire, `self.market.quote(...)` returns the most recent value, not the last one that happened to clear the throttle.

Throttle is declared per-callback at registration:

```python
dispatcher.register_quote("binance", self.bsym, self._on_signal,
                          min_interval_ms=50)         # tight throttle for the signal sampler
dispatcher.register_quote("polymarket", self.window.base_slug,
                          self._on_decision,
                          min_interval_ms=0)          # opt out of the 2s default
```

The throttle key is `(kind, exchange, symbol, callback id)`, so two callbacks registered on the same topic don't interfere with each other's cadence. Order/fill/rollover events bypass throttling entirely.

### 5. MarketState as a first-class object (not "the feed's cache")

The earlier design left the live snapshot dict inside `MarketFeed`, and strategies called `feed.latest(symbol, exchange=...)`. That coupled the strategy to the transport: a strategy depending on `MarketFeed` is really depending on "the thing that happens to hold the cache", which is the wrong abstraction.

`MarketState` (`market/state.py`) is now the dedicated owner of cross-source live data:

```python
self.market.quote("binance", "btcusdt")        # latest Quote, or None
self.market.mid("polymarket", "btc-updown-15m")  # falls back to (bid+ask)/2
self.market.recent_trades("binance", "btcusdt", n=20)  # bounded history
self.market.current_asset_id("polymarket", base_slug)  # rolling-topic helper
self.market.snapshot_all()                     # entire cross-source dict
```

The dispatcher writes via private `_update_*` methods; strategy callbacks read via the public methods. `MarketFeed` is reduced to transport: it owns the ZMQ sockets, decodes msgpack, and calls the producer hook the engine passed in. Bounded trade history (default 64 prints per `(ex, sym)`) is in MarketState too — strategies that want a moving window read it instead of maintaining their own deque.

### 6. Strategy declares its data deps up front

`required_topics()` returns the list of `(exchange, symbol, kind)` tuples the strategy needs. The engine reads it *before* `setup()` runs and issues the ZMQ subscriptions:

```python
def required_topics(self):
    return [
        ("polymarket", self.window.base_slug, "snapshot"),
        ("polymarket", self.window.base_slug, "trade"),
        ("binance",    "btcusdt",             "snapshot"),
    ]
```

Two reasons for the up-front declaration:

- **Self-documenting.** A reader sees the strategy's data dependencies in one place without grep-ing the body.
- **Subscriptions are not events.** A `dispatcher.register_quote(...)` call only adds a routing-table entry — it does not subscribe a ZMQ topic. If you forget to list a topic in `required_topics()`, the engine never tells the server it wants frames for that topic, and your callback silently never fires.

### 7. Rollover is an event, not a special case

Polymarket up/down markets roll every 15 minutes (server-side); Kalshi series roll per-ticker-expiry. The server stamps `full_slug` into every snapshot/trade payload. `MarketFeed` tracks the last-seen `full_slug` per `(exchange, base_slug)` and emits a `RolloverEvent` whenever it changes — including the very first frame (bootstrap fill).

Strategies receive rollover as a normal callback:

```python
def setup(self, dispatcher):
    dispatcher.register_rollover("polymarket", self.window.base_slug,
                                 self._on_rollover)

def _on_rollover(self, full_slug, asset_id):
    self.window.full_slug = full_slug
    self.window.up_asset_id = asset_id
    # Window-scoped state resets:
    self._live_oid = None
    self._filled_this_window = False
```

The event fires *before* the first quote of the new window arrives — so by the time `_on_quote` runs, `self.window.up_asset_id` is already pointing at the new asset.

Pre-start frames (the ~10s overlap when the new server-side window is warming up) are gated server-side and never reach the engine. The engine doesn't filter for them.

## Prerequisites

The engine talks to two already-running services (three when placing orders). Bring them up first (host-native or via docker):

| Service          | Endpoint                                      | Source                          | Needed for           |
|------------------|-----------------------------------------------|---------------------------------|----------------------|
| zmq_server       | `tcp://127.0.0.1:5555` (market PUB)           | `cargo run --bin orderbook_server` | all strategies    |
| zmq_server       | `tcp://127.0.0.1:5556` (control ROUTER)       | (same)                          | static-exchange feeds (binance/binance_spot/okx) |
| zmq_server       | `tcp://127.0.0.1:5559` (user PUB)             | (same)                          | trading strategies   |
| executor         | `tcp://127.0.0.1:5557` (order ROUTER)         | `cargo run -p executor`         | trading strategies   |

`observe` runs against a stack with no executor.

Easiest path: `make up` (see repo root README).

Python deps:

```bash
pip install pyzmq msgpack requests
```

Run from the **repo root** so the package import resolves cleanly.

## Class reference

The four classes you actually need to understand to write a strategy.

### `Dispatcher` — `trading/dispatcher.py`

Single-threaded loop draining a `queue.Queue[Event]`. Owned by `Engine`; passed to `Strategy.setup()` as the registration surface. Public API:

```python
# min_interval_ms defaults to DEFAULT_MIN_INTERVAL_MS (2000.0).
# Pass 0 to fire on every event; pass a smaller value for a tighter throttle.
dispatcher.register_quote(exchange, symbol, cb, *, min_interval_ms=2000.0)
dispatcher.register_trade(exchange, symbol, cb, *, min_interval_ms=2000.0)
dispatcher.register_rollover(exchange, base_slug, cb)
dispatcher.register_order_update(cb)
dispatcher.register_fill(cb)
dispatcher.stop()    # enqueue ShutdownEvent; safe from any thread
dispatcher.run()     # blocks until ShutdownEvent
```

Throttling is per-registration: each `register_*` call creates a `_Registration` carrying `min_interval_ns` + `last_fired_ns`. The same `(exchange, symbol)` can be registered multiple times by different callbacks with different throttles — useful when one path needs every event (decision) and another needs an infrequent log line. Order, fill, and rollover events bypass throttling entirely.

Exceptions raised by a callback are caught and logged — one buggy callback cannot take down the dispatcher.

### `MarketState` — `market/state.py`

The strategy's single read interface for live data. Single-writer (dispatcher) / single-reader (strategy callbacks on the dispatcher thread) → no locks.

```python
# point reads
self.market.quote(exchange, symbol)         -> Optional[Quote]
self.market.trade(exchange, symbol)         -> Optional[Trade]
self.market.mid(exchange, symbol)           -> Optional[float]  # Quote.mid or (bid+ask)/2
self.market.best_bid(exchange, symbol)      -> Optional[float]
self.market.best_ask(exchange, symbol)      -> Optional[float]

# history (bounded; default maxlen=64 per (ex, sym))
self.market.recent_trades(exchange, symbol, n) -> list[Trade]

# bulk
self.market.snapshot_all()                  -> dict[(ex, sym), Quote]
self.market.trades_all()                    -> dict[(ex, sym), Trade]

# rolling-topic helper
self.market.current_asset_id(exchange, base_slug) -> Optional[str]
```

`_update_quote` / `_update_trade` / `_update_rollover` are private — only `Dispatcher._dispatch` calls them, always before firing user callbacks.

### `Strategy` — `trading/strategies/base.py`

The contract a strategy fills in.

```python
class Strategy(abc.ABC):
    name: str = "abstract"

    def __init__(self, *, market: MarketState, router: OrderRouter,
                 state: StrategyState, window: Optional[MarketWindow] = None):
        ...

    def required_topics(self) -> list[tuple[str, str, str]]:
        # (exchange, symbol, kind="snapshot"|"trade") tuples the engine
        # MUST subscribe before setup() runs. If a topic isn't here,
        # no callback you register against it will ever fire.
        return []

    @abc.abstractmethod
    def setup(self, dispatcher: Dispatcher) -> None:
        # Register all event callbacks here.

    def teardown(self) -> None:
        # Cancel any live orders on shutdown. Default no-op.

    @property
    def label(self) -> str:
        # Human-readable tag for log lines. Falls back to base_slug
        # when full_slug is None (bootstrap window).
```

`router` is `None` for observe-shaped strategies (the engine skips both `OrderRouter` and `UserFeed` when `--strategy observe`).

### `StrategyState` + `OrderLedger` — `typings/state.py`

Per-strategy bookkeeping. Each `Engine` builds one `StrategyState` and passes it to the strategy.

```python
state.orders                        # OrderLedger — see below
state.trades: Deque[Trade]          # bounded trade history the strategy chose to record
state.scratch: dict[str, Any]       # free-form per-strategy stash

state.orders.add(OrderRecord(...))                  # after a place ack
state.orders.bind_exchange_oid(client_oid, oid)     # if you placed without oid first
state.orders.mark(order_id, status)                 # in on_order_update
state.orders.live() -> list[OrderRecord]            # not-terminal orders
state.orders.count_opened() -> int                  # lifetime opens this strategy
```

`OrderRecord` carries: `client_oid`, `side_name` (free-form: "up", "up-exit", etc.), `symbol`, `direction` ("buy"|"sell"), `px`, `qty`, optional `exchange_oid`, `status` (`placed`→`live`→terminal), `filled_qty`. `is_terminal` is True when `status ∈ {filled, canceled, rejected, expired}`.

The ledger is what replaced the old per-strategy `filled_this_window` boolean. `fill_once` checks `state.orders.live()` (with terminal filter) to know whether it's already filled this window.

## Strategies

Selected with `--strategy <name>`. Five names:

| Strategy    | Places? | Fills? | Terminates? | Use for                                                |
|-------------|---------|--------|-------------|--------------------------------------------------------|
| `observe`   | no      | no     | on Ctrl-C   | Multi-exchange orderbook watcher; verify market data   |
| `probe`     | yes     | no     | on Ctrl-C   | Far-from-touch place + cancel; verify the order pipe   |
| `one_shot`  | yes     | no     | auto        | Single place + cancel, then exits cleanly              |
| `fill_once` | yes     | yes    | on Ctrl-C   | Cross the spread; fill once per window                 |
| `momentum`  | yes     | yes    | on Ctrl-C   | Binance signal → one small Polymarket position (TP/SL) |

Legacy `--step 0/1/2` still works (`0→observe`, `1→probe`, `2→fill_once`) but prints a deprecation warning.

### Single-market constraint

`probe` / `fill_once` / `one_shot` / `momentum` each operate on **one** Polymarket window — they require exactly one value for `--base-slugs`:

```bash
python -m scripts.trading_engine --strategy probe --base-slugs btc-updown-15m
# two markets? two processes:
python -m scripts.trading_engine --strategy probe --base-slugs btc-updown-15m &
python -m scripts.trading_engine --strategy probe --base-slugs eth-updown-15m &
```

Passing more than one value causes the engine to exit at startup with a clear error. `observe` is the exception — it accepts the full list and watches every (exchange, symbol) you pass.

### `observe` — orderbook only (no orders)

Verifies the market-data path end-to-end before any orders fly. Subscribes Binance, Binance Spot, Polymarket, and Kalshi via the `zmq_server` market PUB. Per-topic status lines arrive at most every ~5s; a unified state table dumps at most every ~5s.

```bash
python -m scripts.trading_engine --strategy observe
```

No connection to the executor, no orders, no user-channel feed. Safe to run against a stack with no executor at all. Rollovers show up as fresh `full_slug` values on the per-topic status line.

Override tracked symbols / series with `--binance-symbols`, `--binance-spot-symbols`, `--base-slugs`, `--kalshi-series`.

### `probe` — place + cancel on every quote (no fills)

On every Polymarket UP quote: place a $0.01 buy at the **absolute price floor**, then cancel synchronously in the same callback. Two safety properties:

- **Price = $0.01 (the tick floor).** Below every standing bid → the matching engine cannot match against any resting sell → no partial fills.
- **Cancel is immediate.** The cancel REST call fires inside the same callback, on the same dispatcher thread — no other event can interleave. Order's lifetime on the book is bounded by two REST round-trips (~hundreds of ms).

Mid outside `[--safe-mid-low, --safe-mid-high]` (default `[0.30, 0.70]`) still skips.

```bash
python -m scripts.trading_engine --strategy probe --base-slugs btc-updown-15m
```

Watch:

- Engine logs: `PROBE place px=… oid=…` and `PROBE cancel ok oid=…` per quote.
- `python -m scripts.trading_engine.io.user_feed` (separate shell): `order_update` with `status=new` after each place, `status=canceled` after the immediate cancel.
- Frontend → Account → Orders: same events arrive within ~2s.

Ctrl-C: `teardown()` cancels any stashed oid before exiting.

### `one_shot` — place once, cancel once, exit

Single Polymarket quote callback. On first valid quote: place a $0.01 buy at the price floor, cancel synchronously, then call `dispatcher.stop()` so the engine exits cleanly.

Same safety properties as `probe` (floor price + same-callback cancel). Engine-managed equivalent of the old `poly_place_cancel.py`, but picks up the current Polymarket window automatically.

```bash
python -m scripts.trading_engine --strategy one_shot --base-slugs btc-updown-15m
```

### `fill_once` — fill exactly once per window

Quotes ask + 1 tick (crosses) so the order takes immediately. `$10 / px` tokens per order. After a fill arrives, the strategy parks itself until the next window (`_on_rollover` clears the latch).

```bash
python -m scripts.trading_engine --strategy fill_once --base-slugs btc-updown-15m --order-notional-usd 10
```

Watch for `FILL_ONCE filled px=… qty=…` in the engine logs and `user.polymarket.fill` on the user-feed listener / frontend.

### `momentum` — Binance signal → one small Polymarket position

Two registered callbacks:

- **Binance quote (throttled ~50ms)** — keeps a samples deque current for the momentum signal. The throttle prevents 200×/sec fast quotes from triggering 200 strategy invocations; `MarketState.mid(...)` always stays fresh between fires because state is updated unconditionally before the throttle check.
- **Polymarket quote (no throttle — `min_interval_ms=0` override)** — runs the entry / exit / manage-open decision tree. The "force-exit in last N secs of the window" check fires here, since it's gated on Polymarket quotes anyway and no `on_timer` event exists in this engine.

Window-phase guard: no new entry in the first `SKIP_FIRST_SECS=60` or the last `SKIP_LAST_SECS=120` of the window; in the last 120s any open position is force-closed.

```bash
python -m scripts.trading_engine --strategy momentum --base-slugs btc-updown-15m
```

Run small. The tunables (`MOMENTUM_LOOKBACK_SECS`, `MOMENTUM_THRESHOLD_BPS`, `TP_PCT`, `SL_PCT`, …) are hardcoded in `strategies/momentum.py` — one place to tune.

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

### User-event listener — tail fills + order_updates

```bash
python -m scripts.trading_engine.io.user_feed
python -m scripts.trading_engine.io.user_feed --topic-prefix user.polymarket.fill
```

SUBs the `zmq_server` user PUB and pretty-prints every event. Useful as a side-by-side console while a strategy runs.

## Common flags

| Flag                     | Default                                  | Meaning                                                                       |
|--------------------------|------------------------------------------|-------------------------------------------------------------------------------|
| `--strategy`             | `probe`                                  | `observe` / `probe` / `one_shot` / `fill_once` / `momentum`                   |
| `--step`                 | _(deprecated)_                           | Legacy: `0→observe`, `1→probe`, `2→fill_once`. Prints a warning.              |
| `--base-slugs`           | `btc-updown-15m eth-updown-15m`          | Polymarket base slugs. Window strategies require exactly **one**.             |
| `--kalshi-series`        | `KXBTC15M KXETH15M`                      | Kalshi series to track (observe only)                                        |
| `--binance-symbols`      | `btcusdt ethusdt`                        | Binance USDT-M futures symbols (observe only)                                |
| `--binance-spot-symbols` | `btcusdt ethusdt`                        | Binance spot symbols (observe only)                                          |
| `--order-notional-usd`   | `10.0`                                   | Per-order notional cap (`fill_once`)                                          |
| `--safe-mid-low`         | `0.30`                                   | Skip placing on a side whose mid is below this                                |
| `--safe-mid-high`        | `0.70`                                   | Skip placing on a side whose mid is above this                                |
| `--backend-url`          | `http://127.0.0.1:3000`                  | Backend HTTP host (reserved; current strategies don't poll the backend)       |
| `--market-pub`           | `tcp://127.0.0.1:5555`                   | `zmq_server` market PUB                                                       |
| `--market-router`        | `tcp://127.0.0.1:5556`                   | `zmq_server` control-plane ROUTER (handshake for static exchanges)            |
| `--user-pub`             | `tcp://127.0.0.1:5559`                   | `zmq_server` user PUB                                                         |
| `--router-endpoint`      | `tcp://127.0.0.1:5557`                   | Executor order ROUTER                                                         |
| `--engine-log-dir`       | `/syslog/trading_engine/`                | Directory for the append-only action + event log (daily-rotated CSV). **Mandatory** — engine refuses to start if this dir isn't writable. |
| `--log-level`            | `info`                                   | `debug` / `info` / `warning` / `error`                                        |

`--tick-secs` / `--rediscover-secs` / `--report-secs` are **gone** — the engine has no tick loop or backend poller, and observe's report cadence is internal.

## Recipes

**Just BTC 15m, debug logs:**

```bash
python -m scripts.trading_engine --strategy probe --base-slugs btc-updown-15m --log-level debug
```

**One-time place + cancel sanity check (auto-exits):**

```bash
python -m scripts.trading_engine --strategy one_shot --base-slugs btc-updown-15m
```

**Smaller order size ($2 each):**

```bash
python -m scripts.trading_engine --strategy fill_once --base-slugs btc-updown-15m --order-notional-usd 2
```

**Two markets in parallel:**

```bash
python -m scripts.trading_engine --strategy probe --base-slugs btc-updown-15m --engine-log-dir /syslog/trading_engine/btc &
python -m scripts.trading_engine --strategy probe --base-slugs eth-updown-15m --engine-log-dir /syslog/trading_engine/eth &
```

> Each parallel engine MUST have its own `--engine-log-dir`. Two engines writing into the same dir would interleave action rows for different markets in the same `actions/<date>.csv` and force-flip the headers as they rotate.

**Point at a remote stack (e.g. docker on another host):**

```bash
python -m scripts.trading_engine --strategy probe --base-slugs btc-updown-15m --backend-url http://10.0.0.5:3000 --market-pub tcp://10.0.0.5:5555 --market-router tcp://10.0.0.5:5556 --user-pub tcp://10.0.0.5:5559 --router-endpoint tcp://10.0.0.5:5557
```

> Note: the user PUB binds on `tcp://*:5559` inside the orderbook_server container; docker-compose maps `127.0.0.1:5559` on the host, so the default `--user-pub tcp://127.0.0.1:5559` works out of the box.

## Safety behaviours built in

- **Pinned-floor guard** (`helpers.safe_mid_guard`). If a side's `best_bid` is at or below the tick floor ($0.01), the strategy doesn't place on that quote. Configurable via the `pin_px=` kwarg passed to each strategy.
- **Safe-mid band**. Strategies skip when the mid is outside `[safe_mid_low, safe_mid_high]` (default `[0.30, 0.70]`).
- **Idempotent placement** (`fill_once`). If the live quote already matches target px+qty, the callback is a no-op (no API churn).
- **Soft cancels.** `NotFound` cancels are treated as success in `OrderRouter.cancel` (order already terminated).
- **Graceful shutdown.** SIGINT/SIGTERM enqueues a `ShutdownEvent`; the dispatcher drains the rest of the queue, then `teardown()` cancels any live orders the strategy is tracking.
- **Rollover handling.** `RolloverEvent` fires before the first quote of the new window arrives; the strategy resets its window-scoped state (live oids, `filled_this_window`, position) and trades the new asset_id without a restart.
- **Pre-start gating** (server-side, unchanged). The new Polymarket / Kalshi window doesn't publish to ZMQ or write Redis until its boundary, so the engine never sees a pre-start frame.

## Module layout

```
scripts/trading_engine/
├── __init__.py             # exports the public API
├── __main__.py             # python -m scripts.trading_engine
├── app.py                  # Engine + main()
├── cli.py                  # parse_args()
│
├── io/                     # transport / external I/O
│   ├── executor_client.py  # low-level ZMQ DEALER + msgpack helpers
│   ├── order_router.py     # strategy-friendly wrapper over ExecutorClient
│   ├── user_feed.py        # ZMQ SUB on user PUB → callback
│   └── event_log.py        # daily-rotated JSONL action + event log
│
├── market/                 # market-data
│   ├── feed.py             # ZMQ SUB on market PUB; producer callbacks enqueue Events
│   └── state.py            # MarketState — live cross-source quotes/trades/mid/history
│
├── typings/                # pure data
│   ├── events.py           # QuoteEvent / TradeEvent / RolloverEvent / FillEvent / OrderUpdateEvent / ShutdownEvent
│   ├── state.py            # StrategyState + OrderLedger + OrderRecord
│   └── window.py           # PolymarketWindow / KalshiWindow
│
├── utils/                  # helpers
│   └── windows.py          # HTTP discovery of active Polymarket / Kalshi windows (used by external callers)
│
└── trading/                # active logic
    ├── dispatcher.py       # Dispatcher — single-threaded event router
    ├── helpers.py          # MIN_PX/PIN_PX/SAFE_MID_* + clamp_px + qty_for_notional + safe_mid_guard
    └── strategies/
        ├── __init__.py     # make_strategy(name, ...) + available_strategies()
        ├── base.py         # Strategy abstract base (required_topics / setup / teardown)
        ├── observe.py      # ObserveStrategy
        ├── probe.py        # ProbeStrategy
        ├── fill_once.py    # FillOnceStrategy
        ├── one_shot.py     # OneShotStrategy
        └── momentum.py     # MomentumStrategy
```

Every executor-facing tool lives inside this package. The standalone scripts that used to sit at `scripts/poly_*.py` have been folded in:

| Old standalone script         | Replaced by                                                            |
|-------------------------------|------------------------------------------------------------------------|
| `scripts/poly_heartbeat.py`    | `python -m scripts.trading_engine.io.order_router`                     |
| `scripts/poly_user_listener.py`| `python -m scripts.trading_engine.io.user_feed`                        |
| `scripts/poly_place_cancel.py` | `python -m scripts.trading_engine --strategy one_shot`                 |
| `scripts/executor_client.py`   | `from scripts.trading_engine.io import ExecutorClient, place, cancel`  |

### Adding a strategy

1. Create `trading/strategies/<name>.py` with a class subclassing `Strategy` from `.base`.
2. Set `name = "<name>"` on the class.
3. Implement `required_topics()` — return the list of `(exchange, symbol, kind)` tuples (`kind` ∈ `"snapshot"`, `"trade"`) the engine should subscribe before your callbacks fire.
4. Implement `setup(dispatcher)` — register your callbacks:

   ```python
   def setup(self, dispatcher):
       dispatcher.register_quote("binance", "btcusdt", self._on_signal, min_interval_ms=50)
       dispatcher.register_quote("polymarket", self.window.base_slug, self._on_poly_quote)
       dispatcher.register_rollover("polymarket", self.window.base_slug, self._on_rollover)
       dispatcher.register_fill(self._on_fill)
       dispatcher.register_order_update(self._on_order_update)
   ```

5. (Optional) Override `teardown()` to cancel any live orders on shutdown.
6. Register the class in `trading/strategies/__init__.py`'s `_REGISTRY`.

`--strategy <name>` will then pick it up; `available_strategies()` reflects the registry. Read `MarketState` (`self.market.quote(...)`, `self.market.mid(...)`, `self.market.recent_trades(...)`) inside callbacks instead of re-implementing per-strategy caches — every event updates state before any callback fires, so reads are always fresh.

## Known limitations

- Every strategy currently quotes **buy** on the Polymarket UP side only. To test the sell path, change `side="buy"` in the relevant strategy file.
- No startup reconciliation against `GET /api/orders` — orders left from a previous engine run aren't tracked. Fire the executor's kill switch to clear them: `docker compose exec redis redis-cli SET executor:cancel_all 1`.
- Geoblock applies: the executor process must egress from a Polymarket-allowed region. If running natively, your host VPN covers it; if running in docker, the container is on the bridge network and needs a VPN sidecar (see repo root README troubleshooting).
- Trading strategies operate on **one** Polymarket window per process. This is intentional (simpler dispatcher, no in-process strategy fan-out) — run multiple processes for multiple markets.
