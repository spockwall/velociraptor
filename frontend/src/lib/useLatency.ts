import { useRef, useState, useCallback } from "react";

/** Max samples kept in the rolling window. */
const WINDOW = 100;

/** One latency-bearing payload: an exchange time + a local receive time, both
 *  in Unix nanoseconds. `ex_timestamp === 0` means the venue sent no usable
 *  timestamp, so the sample is skipped (latency would be meaningless). */
export interface LatencySample {
    ex_timestamp: number;
    recv_timestamp: number;
}

/** Average data latency (recv − ex, in ms) over the last 100 *distinct*
 *  payloads. `dedupeKey` (e.g. `sequence`) prevents the same snapshot from
 *  being counted on every poll; pass `undefined` to push every sample
 *  (used by the trades list, where each row is already distinct).
 *
 *  Returns `{ push, avgMs, count }`:
 *    - `push(sample, key?)` — record one payload; no-op when `ex_timestamp`
 *      is 0 or `key` matches the previous one.
 *    - `avgMs` — mean latency over the window, or `null` when empty.
 *    - `count` — how many samples currently back the average.
 */
export function useLatency() {
    const buf = useRef<number[]>([]);
    const lastKey = useRef<number | string | undefined>(undefined);
    const [stats, setStats] = useState<{ avgMs: number | null; count: number }>({
        avgMs: null,
        count: 0,
    });

    const push = useCallback((s: LatencySample, key?: number | string) => {
        // No venue timestamp → no measurable venue→receive latency.
        if (!s.ex_timestamp || !s.recv_timestamp) return;
        // Skip re-counting the same payload across polls.
        if (key !== undefined && key === lastKey.current) return;
        lastKey.current = key;

        const ms = (s.recv_timestamp - s.ex_timestamp) / 1e6;
        // Guard against clock skew producing absurd values; keep plausible ones.
        if (!Number.isFinite(ms)) return;

        const arr = buf.current;
        arr.push(ms);
        if (arr.length > WINDOW) arr.shift();
        const avg = arr.reduce((a, b) => a + b, 0) / arr.length;
        setStats({ avgMs: avg, count: arr.length });
    }, []);

    return { push, avgMs: stats.avgMs, count: stats.count };
}
