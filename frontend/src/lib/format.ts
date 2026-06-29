export function fmtPrice(v: number | null | undefined, decimals = 4): string {
    if (v == null) return "—";
    return v.toLocaleString("en-US", { minimumFractionDigits: decimals, maximumFractionDigits: decimals });
}

export function fmtQty(v: number | null | undefined, decimals = 4): string {
    if (v == null) return "—";
    if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(2)}M`;
    if (v >= 1_000) return `${(v / 1_000).toFixed(2)}K`;
    return v.toFixed(decimals);
}

/** Format a timestamp as HH:MM:SS. Accepts either an RFC3339 string or a
 *  Unix-nanosecond number (the wire now ships `recv_timestamp` as i64 ns). */
export function fmtTs(ts: string | number): string {
    const ms = typeof ts === "number" ? ts / 1e6 : Date.parse(ts);
    if (!Number.isFinite(ms)) return "—";
    return new Date(ms).toLocaleTimeString("en-US", { hour12: false });
}

/** Format a latency in milliseconds for display. `null` → em dash. */
export function fmtLatency(ms: number | null): string {
    if (ms == null || !Number.isFinite(ms)) return "—";
    if (ms < 1) return `${(ms * 1000).toFixed(0)}µs`;
    if (ms < 1000) return `${ms.toFixed(1)}ms`;
    return `${(ms / 1000).toFixed(2)}s`;
}

export function fmtSpread(v: number | null | undefined): string {
    if (v == null) return "—";
    return v.toFixed(6);
}

export function fmtPct(v: number): string {
    return `${(v * 100).toFixed(2)}%`;
}

export function relativeTime(iso: string): string {
    const diff = Date.now() - new Date(iso).getTime();
    if (diff < 1000) return "just now";
    if (diff < 60_000) return `${Math.floor(diff / 1000)}s ago`;
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
    return `${Math.floor(diff / 3_600_000)}h ago`;
}
