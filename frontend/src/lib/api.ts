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

export const api = {
    health: () => get<{ ok: boolean }>("/health"),

    orderbook: (exchange: string, symbol: string) => get<OrderbookSnapshot>(`${BASE}/orderbook/${exchange}/${symbol}`),

    bba: (exchange: string, symbol: string) => get<BbaPayload>(`${BASE}/bba/${exchange}/${symbol}`),

    snapshots: (exchange: string, symbol: string, limit = 20) =>
        get<OrderbookSnapshot[]>(`${BASE}/snapshots/${exchange}/${symbol}?limit=${limit}`),

    trades: (exchange: string, symbol: string, limit = 50) =>
        get<LastTradePrice[]>(`${BASE}/trades/${exchange}/${symbol}?limit=${limit}`),
};
