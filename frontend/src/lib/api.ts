const BASE = "/api";

export interface OrderbookSnapshot {
    exchange: Record<string, number> | string;
    symbol: string;
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
    sequence: number;
    timestamp: string;
    best_bid: [number, number] | null;
    best_ask: [number, number] | null;
    spread: number | null;
}

export interface LastTradePrice {
    exchange: Record<string, number> | string;
    symbol: string;
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

    /// Latest spot price from a CEX, used as a temporary priceToBeat while
    /// Polymarket's official one hasn't been published yet. Always fresh.
    spotPrice: (product: string, source: "coinbase" | "kraken" | "binance" = "kraken") =>
        get<{ product: string; price: number; source: string; ts: number }>(
            `${BASE}/spot_price/${product}?source=${source}`
        ),

    /// Spot price *snapshotted at window-open time* — the backend caches the
    /// first fetch for each `(product, interval_secs, window_start)` in redis
    /// (24h TTL). Including `interval_secs` keeps 5m and 15m windows
    /// independent even when their boundaries coincide.
    windowOpenPrice: (
        product: string,
        intervalSecs: number,
        windowStart: number,
        source: "coinbase" | "kraken" | "binance" = "kraken"
    ) =>
        get<{
            product: string;
            interval_secs: number;
            window_start: number;
            price: number;
            source: string;
            ts: number;
        }>(
            `${BASE}/window_open_price/${product}/${intervalSecs}/${windowStart}?source=${source}`
        ),
};
