const BASE = "/api";

export interface OrderbookSnapshot {
    exchange: Record<string, number> | string;
    symbol: string;
    /** For rolling markets, the current window's full_slug (e.g.
     *  `btc-updown-15m-1715423400`). `null`/absent for static exchanges. */
    full_slug?: string | null;
    sequence: number;
    /** Exchange-stamped time in Unix nanoseconds (`0` when the venue sends
     *  none — e.g. Binance spot depth, Kalshi snapshot). */
    ex_timestamp: number;
    /** Local receive time in Unix nanoseconds (always populated). */
    recv_timestamp: number;
    best_bid: [number, number] | null;
    best_ask: [number, number] | null;
    spread: number | null;
    mid: number | null;
    wmid: number;
    bids: [number, number][];
    asks: [number, number][];
}

export interface BbaPayload {
    exchange: string;
    symbol: string;
    /** Mirrors `OrderbookSnapshot.full_slug` for rolling markets. */
    full_slug?: string | null;
    sequence: number;
    /** Exchange-stamped time in Unix nanoseconds (`0` when unavailable). */
    ex_timestamp: number;
    /** Local receive time in Unix nanoseconds. */
    recv_timestamp: number;
    best_bid: [number, number] | null;
    best_ask: [number, number] | null;
    spread: number | null;
}

export interface LastTradePrice {
    exchange: Record<string, number> | string;
    symbol: string;
    /** Mirrors `OrderbookSnapshot.full_slug` for rolling markets. */
    full_slug?: string | null;
    price: number;
    size: number;
    side: string;
    fee_rate_bps: number;
    market: string;
    /** Exchange-stamped trade time in Unix nanoseconds (`0` if the venue
     *  sends none). */
    ex_timestamp: number;
    /** Local receive time in Unix nanoseconds. */
    recv_timestamp: number;
}

function resolveExchange(e: Record<string, number> | string): string {
    if (typeof e === "string") return e;
    return Object.keys(e)[0] ?? "";
}

export function exchangeName(snap: OrderbookSnapshot | LastTradePrice): string {
    return resolveExchange(snap.exchange as Record<string, number> | string);
}

async function get<T>(path: string): Promise<T> {
    const res = await fetch(path);
    if (!res.ok) {
        const body = await res.json().catch(() => ({ error: res.statusText }));
        throw new Error(body.error ?? res.statusText);
    }
    return res.json() as Promise<T>;
}

async function post<T>(path: string, payload: object): Promise<T> {
    const res = await fetch(path, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
    });
    if (!res.ok) {
        const body = await res.json().catch(() => ({ error: res.statusText }));
        throw new Error(body.error ?? res.statusText);
    }
    return res.json() as Promise<T>;
}

export interface ControlStatus {
    kill_switch: boolean;
    deadman_engaged: boolean;
    last_heartbeat_secs: number;
}

export type ControlAction = { type: "halt" } | { type: "resume" } | { type: "reload_risk" };

// System monitor — mirrors `backend/src/routes/monitor.rs`.

export interface HostInfo {
    hostname: string;
    os: string;
    kernel: string;
    uptime_secs: number;
    cpu_count: number;
}

export interface CpuInfo {
    usage_pct: number;
    per_core_pct: number[];
    load_avg: [number, number, number];
}

export interface MemoryInfo {
    total_bytes: number;
    used_bytes: number;
    available_bytes: number;
    used_pct: number;
    swap_total_bytes: number;
    swap_used_bytes: number;
}

export interface DiskInfo {
    mount_point: string;
    total_bytes: number;
    available_bytes: number;
    used_bytes: number;
    used_pct: number;
}

export interface ServiceInfo {
    unit: string;
    active_state: string;
    sub_state: string;
    load_state: string;
    main_pid: number;
    active_secs: number;
    error: string | null;
}

export interface DataDirUsage {
    /** Immediate subfolder name under the configured data_dir. */
    name: string;
    /** Recursive total size of the subfolder, in bytes. */
    size_bytes: number;
    /** Share of the data_dir filesystem's total capacity, 0..100. */
    pct_of_fs: number;
}

export interface MonitorStatus {
    host: HostInfo;
    cpu: CpuInfo;
    memory: MemoryInfo;
    disks: DiskInfo[];
    /** du-style usage per immediate subfolder of the configured data_dir
     *  (e.g. /data). Empty when the path doesn't exist (dev box). */
    data_usage: DataDirUsage[];
    services: ServiceInfo[];
    ts: number;
}

// Redis key inventory — mirrors `libs::redis_client::RedisKeyInfo`.
export interface RedisKeyInfo {
    key: string;
    /** string / list / set / hash / zset / stream / none. */
    kind: string;
    /** Element count for list/set/hash/zset/stream; null for strings. */
    size: number | null;
    /** Decoded payload for bba: (BbaPayload) and window_open_price: (JSON).
     *  Absent for key families we don't decode. */
    data?: {
        // bba: fields
        best_bid?: [number, number] | null;
        best_ask?: [number, number] | null;
        spread?: number | null;
        // window_open_price: fields
        product?: string;
        interval_secs?: number;
        window_start?: number;
        price?: number;
        source?: string;
        [k: string]: unknown;
    } | null;
}

// Error logs — mirrors `backend/src/routes/logs.rs::LogEntry`.

export interface LogEntry {
    /** Which service's `.error.log` this came from. */
    service: string;
    /** Parsed RFC3339 timestamp, or null when the line didn't start with one. */
    ts: string | null;
    /** Parsed level (WARN / ERROR), or null. */
    level: string | null;
    /** Parsed tracing target (e.g. `backend::routes`), or null. */
    target: string | null;
    /** The full original log line (always present). */
    raw: string;
}

export interface PolymarketMarket {
    /** Stable identifier across rollovers. The UI keys cards on this and
     *  fetches orderbook via `/api/orderbook/polymarket/{base_slug}`. */
    base_slug: string;
    /** Per-side asset_id from the backend (one label per up/down). The UI
     *  filters to `side === "up"` so each market shows as one card. */
    asset_id: string;
    /** Current window's full_slug. Changes on every rollover. */
    full_slug: string;
    /** "up" or "down" — the UI keeps only the UP entries. */
    side: string;
    window_start: number;
    interval_secs: number;
    title: string;
}

export interface KalshiMarket {
    ticker: string;
    series: string;
    window_start: number;
    interval_secs: number;
    title: string;
}

// UserEvent — internally tagged on the wire. Discriminator is `type`.
// See libs/src/protocol/events.rs. Mirrors the Rust enum 1:1.

export type Side = "buy" | "sell";

export type OrderStatus =
    | "new"
    | "partially_filled"
    | "filled"
    | "canceled"
    | "rejected"
    | "expired";

/// One fill — a single trade matched against (potentially several) makers.
/// `taker_oid` is the taker's order id; `client_oid` is whatever client_oid
/// the taker passed at place time, when known. `maker_orders` is the raw
/// exchange-specific maker-list payload (Polymarket today).
///
/// `trade_status` is the venue's trade-lifecycle string. Polymarket emits
/// `MATCHED`, `MINED`, `CONFIRMED`, `RETRYING`, or `FAILED` for on-chain
/// settlement progress. `null` for venues that don't publish one.
export interface UserFill {
    type: "fill";
    exchange: string;
    taker_oid: string | null;
    client_oid: string | null;
    exchange_oid: string;
    symbol: string;
    side: Side;
    px: number;
    qty: number;
    fee: number;
    ts_ns: number;
    trade_status: string | null;
    maker_orders: unknown | null;
}

/// One order-lifecycle event (new / partially_filled / filled / canceled / …).
export interface UserOrderUpdate {
    type: "order_update";
    exchange: string;
    client_oid: string;
    exchange_oid: string;
    symbol: string;
    side: Side;
    px: number;
    qty: number;
    filled: number;
    status: OrderStatus;
    ts_ns: number;
}

export type UserEvent = UserFill | UserOrderUpdate;

// ── Polymarket Explorer ──────────────────────────────────────────────────────
// Public per-user + market data proxied by the backend from Polymarket's
// data-api / lb-api / gamma-api. All keyed by proxy wallet address.

/// One open position. Mirrors data-api `/positions` row (subset we render).
export interface PmPosition {
    proxyWallet: string;
    asset: string;
    conditionId: string;
    size: number;
    avgPrice: number;
    curPrice: number;
    initialValue: number;
    currentValue: number;
    cashPnl: number;
    percentPnl: number;
    realizedPnl: number;
    title: string;
    slug: string;
    icon: string;
    outcome: string;
    outcomeIndex: number;
    redeemable: boolean;
    endDate: string;
}

/// One resolved/closed position. data-api `/closed-positions`.
export interface PmClosedPosition {
    proxyWallet: string;
    asset: string;
    conditionId: string;
    avgPrice: number;
    totalBought: number;
    realizedPnl: number;
    curPrice: number;
    timestamp: number;
    title: string;
    slug: string;
    icon: string;
    outcome: string;
    outcomeIndex: number;
    endDate: string;
}

/// One on-chain trade. data-api `/trades`.
export interface PmTrade {
    proxyWallet: string;
    side: string;
    asset: string;
    conditionId: string;
    size: number;
    price: number;
    timestamp: number;
    title: string;
    slug: string;
    outcome: string;
    outcomeIndex: number;
    transactionHash: string;
}

/// One activity event. `type` is TRADE/SPLIT/MERGE/REDEEM/REWARD/MAKER_REBATE/
/// YIELD/… `side` is BUY/SELL for trades, empty otherwise.
export interface PmActivity {
    proxyWallet: string;
    timestamp: number;
    type: string;
    conditionId: string;
    size: number;
    usdcSize: number;
    price: number;
    side: string;
    outcome: string;
    outcomeIndex: number;
    title: string;
    slug: string;
    transactionHash: string;
    name: string;
    pseudonym: string;
}

/// Portfolio value — backend flattens upstream `[{user,value}]`.
export interface PmValue {
    user: string;
    value: number;
}

/// One P&L time-series point. `t` = unix seconds, `p` = cumulative P&L (USD).
/// The last point is the current total P&L.
export interface PmPnlPoint {
    t: number;
    p: number;
}

/// One holder within a token group of the data-api `/holders` response.
export interface PmHolderEntry {
    proxyWallet: string;
    name: string;
    pseudonym: string;
    amount: number;
    outcomeIndex: number;
    profileImage: string;
}

/// data-api `/holders` returns one group per token (Yes / No).
export interface PmHolderGroup {
    token: string;
    holders: PmHolderEntry[];
}

/// One leaderboard row (lb-api volume/profit).
export interface PmLeaderboardEntry {
    proxyWallet: string;
    name: string;
    pseudonym: string;
    amount: number;
    profileImage: string;
}

/// Market metrics — the "open interest" substitute (Polymarket has no OI field).
/// Gamma market objects carry liquidity + volume series.
export interface PmMarketMetrics {
    slug: string;
    question?: string;
    liquidity?: number;
    volume?: number;
    volume24hr?: number;
    volume1wk?: number;
    volume1mo?: number;
    bestBid?: number;
    bestAsk?: number;
    lastTradePrice?: number;
    [k: string]: unknown;
}

export const api = {
    health: () => get<{ ok: boolean }>("/health"),

    orderbook: (exchange: string, symbol: string) => get<OrderbookSnapshot>(`${BASE}/orderbook/${exchange}/${symbol}`),

    bba: (exchange: string, symbol: string) => get<BbaPayload>(`${BASE}/bba/${exchange}/${symbol}`),

    snapshots: (exchange: string, symbol: string, limit = 20) =>
        get<OrderbookSnapshot[]>(`${BASE}/snapshots/${exchange}/${symbol}?limit=${limit}`),

    trades: (exchange: string, symbol: string, limit = 50) =>
        get<LastTradePrice[]>(`${BASE}/trades/${exchange}/${symbol}?limit=${limit}`),

    polymarketMarkets: () => get<PolymarketMarket[]>(`${BASE}/polymarket/markets`),

    kalshiMarkets: () => get<KalshiMarket[]>(`${BASE}/kalshi/markets`),

    fills: (limit = 50) => get<UserEvent[]>(`${BASE}/events/fills?limit=${limit}`),

    orders: (limit = 50) => get<UserEvent[]>(`${BASE}/events/orders?limit=${limit}`),

    getControl: () => get<ControlStatus>(`${BASE}/control`),

    monitor: () => get<MonitorStatus>(`${BASE}/monitor`),

    /// Live Redis key inventory (every key + type + element count), fetched
    /// fresh via SCAN. Sorted by key.
    redisKeys: () => get<RedisKeyInfo[]>(`${BASE}/redis/keys`),

    /// Recent monitor samples, newest-first. Backed by a capped Redis list the
    /// backend's sampler writes every 30s. The chart reverses for time order.
    monitorHistory: (limit = 720) =>
        get<MonitorStatus[]>(`${BASE}/monitor/history?limit=${limit}`),

    /// Recent error-log entries across all services, newest-first. Backed by a
    /// capped Redis list the backend's tailer fills from each service's daily
    /// `.error.log`.
    errorLogs: (limit = 200) => get<LogEntry[]>(`${BASE}/logs/errors?limit=${limit}`),

    // Polymarket Explorer — arbitrary-user public data.
    pmResolve: (id: string) =>
        get<{
            wallet: string;
            source: string;
            input?: string;
            name?: string | null;
            pseudonym?: string | null;
        }>(`${BASE}/pm/resolve/${encodeURIComponent(id)}`),
    pmPositions: (wallet: string) => get<PmPosition[]>(`${BASE}/pm/positions/${wallet}`),
    pmClosedPositions: (wallet: string, limit = 100) =>
        get<PmClosedPosition[]>(`${BASE}/pm/closed-positions/${wallet}?limit=${limit}`),
    pmTrades: (wallet: string, limit = 100) =>
        get<PmTrade[]>(`${BASE}/pm/trades/${wallet}?limit=${limit}&takerOnly=true`),
    pmActivity: (wallet: string, type?: string, limit = 100) =>
        get<PmActivity[]>(
            `${BASE}/pm/activity/${wallet}?limit=${limit}${type ? `&type=${type}` : ""}`,
        ),
    pmValue: (wallet: string) => get<PmValue>(`${BASE}/pm/value/${wallet}`),
    pmPnl: (wallet: string, interval: "1d" | "1w" | "1m" | "all" = "all", fidelity: "1h" | "1d" = "1d") =>
        get<PmPnlPoint[]>(`${BASE}/pm/pnl/${wallet}?interval=${interval}&fidelity=${fidelity}`),
    pmHolders: (conditionId: string) => get<PmHolderGroup[]>(`${BASE}/pm/holders/${conditionId}`),
    pmLeaderboard: (metric: "volume" | "profit", window: "all" | "7d" | "30d", limit = 20) =>
        get<PmLeaderboardEntry[]>(
            `${BASE}/pm/leaderboard?metric=${metric}&window=${window}&limit=${limit}`,
        ),
    pmMarket: (slug: string) => get<PmMarketMetrics>(`${BASE}/pm/market/${slug}`),

    postControl: (action: ControlAction) => post<ControlStatus>(`${BASE}/control`, action),

    /// Latest spot price from a CEX, used as a temporary priceToBeat while
    /// Polymarket's official one hasn't been published yet. Always fresh.
    spotPrice: (product: string, source: "coinbase" | "kraken" | "binance" = "kraken") =>
        get<{ product: string; price: number; source: string; ts: number }>(
            `${BASE}/spot_price/${product}?source=${source}`,
        ),

    /// Spot price *snapshotted at window-open time* — the backend caches the
    /// first fetch for each `(product, interval_secs, window_start)` in redis
    /// (24h TTL). Including `interval_secs` keeps 5m and 15m windows
    /// independent even when their boundaries coincide.
    windowOpenPrice: (
        product: string,
        intervalSecs: number,
        windowStart: number,
        source: "coinbase" | "kraken" | "binance" = "kraken",
    ) =>
        get<{
            product: string;
            interval_secs: number;
            window_start: number;
            price: number;
            source: string;
            ts: number;
        }>(`${BASE}/window_open_price/${product}/${intervalSecs}/${windowStart}?source=${source}`),
};
