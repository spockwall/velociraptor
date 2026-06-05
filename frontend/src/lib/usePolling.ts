import { useEffect, useRef, useState, useCallback } from "react";

export function usePolling<T>(fetcher: () => Promise<T>, intervalMs = 1000, enabled = true) {
    const [data, setData] = useState<T | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [loading, setLoading] = useState(true);
    // Unix-seconds timestamp of the last *successful* fetch. Updated inside the
    // fetch callback (an event, not render) so consumers can show "updated Ns
    // ago" without reading the clock during render.
    const [lastUpdated, setLastUpdated] = useState<number | null>(null);
    const timer = useRef<ReturnType<typeof setInterval> | null>(null);

    const fetch = useCallback(async () => {
        try {
            const result = await fetcher();
            setData(result);
            setError(null);
            setLastUpdated(Math.floor(Date.now() / 1000));
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setLoading(false);
        }
    }, [fetcher]);

    useEffect(() => {
        if (!enabled) return;
        fetch();
        timer.current = setInterval(fetch, intervalMs);
        return () => {
            if (timer.current) clearInterval(timer.current);
        };
    }, [fetch, intervalMs, enabled]);

    return { data, error, loading, lastUpdated, refetch: fetch };
}
