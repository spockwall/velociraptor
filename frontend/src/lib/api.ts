const BASE = "/api";

export interface OrderbookSnapshot {
    exchange: Record<string, number> | string;
    symbol: string;
    /** For rolling markets, the current window's full_slug (e.g.
     *  `btc-updown-15m-1715423400`). `null`/absent for static exchanges. */
    full_slug?: string | null;
    sequence: number;
    timestamp: string;
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
    timestamp: string;
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
    timestamp: string;
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

export interface PolymarketMarket {
    asset_id: string;
    base_slug: string;
    full_slug: string;
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
