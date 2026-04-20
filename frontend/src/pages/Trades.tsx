import { useState, useCallback, useMemo } from "react";
import { api, type LastTradePrice, exchangeName } from "../lib/api";
import { fmtPrice, fmtQty, fmtTs } from "../lib/format";
import { usePolling } from "../lib/usePolling";
import Card from "../components/Card";
import Badge from "../components/Badge";
import { PRESETS } from "../components/ExchangeSymbolPicker";
import { AlertCircle, Search } from "lucide-react";

export default function TradesPage() {
    const [limit, setLimit] = useState(50);
    const [search, setSearch] = useState("");

    const fetcher = useCallback(async () => {
        const results = await Promise.all(PRESETS.map((p) => api.trades(p.exchange, p.symbol, limit).catch(() => [])));
        const flat = results.flat();
        // sort newest first
        return flat.sort((a, b) => b.timestamp.localeCompare(a.timestamp));
    }, [limit]);

    const { data: trades, error, loading } = usePolling<LastTradePrice[]>(fetcher, 1200);

    const buys = trades?.filter((t) => t.side === "BUY" || t.side === "buy") ?? [];
    const sells = trades?.filter((t) => t.side === "SELL" || t.side === "sell") ?? [];
    const totalVol = trades?.reduce((a, t) => a + t.size, 0) ?? 0;

    const filteredTrades = useMemo(() => {
        if (!trades) return [];
        if (!search) return trades;
        const q = search.toLowerCase();
        return trades.filter(
            (t) =>
                exchangeName(t).toLowerCase().includes(q) ||
                t.symbol.toLowerCase().includes(q) ||
                t.side.toLowerCase().includes(q),
        );
    }, [trades, search]);

    return (
        <div className="p-5 w-full max-w-6xl mx-auto space-y-6">
            {/* Toolbar */}
            <div className="flex items-center gap-4">
                <div className="relative flex-1 max-w-md">
                    <Search size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-text-muted" />
                    <input
                        type="text"
                        placeholder="Search trades by exchange, symbol, or side..."
                        value={search}
                        onChange={(e) => setSearch(e.target.value)}
                        className="w-full pl-9 pr-4 py-2 rounded-md border border-border-strong bg-bg-surface text-white placeholder:text-text-muted text-xs font-mono focus:outline-none focus:border-border-subtle shadow-sm transition-all"
                    />
                </div>
                <select
                    value={limit}
                    onChange={(e) => setLimit(Number(e.target.value))}
                    className="ml-auto px-4 py-2 rounded-md border border-border-strong bg-bg-surface text-text-muted hover:text-white transition-colors text-xs font-mono shadow-sm outline-none cursor-pointer"
                >
                    {[20, 50, 100, 200].map((n) => (
                        <option key={n} value={n}>
                            {n} rows per symbol
                        </option>
                    ))}
                </select>
            </div>

            {error && (
                <div className="flex items-center gap-2 px-4 py-3 rounded-md border text-xs bg-accent-red/10 border-accent-red/20 text-accent-red shadow-sm">
                    <AlertCircle size={14} />
                    {error}
                </div>
            )}

            {/* Stats row */}
            <div className="grid grid-cols-3 gap-px bg-border-strong rounded-md overflow-hidden shadow-sm">
                {[
                    { label: "total trades", value: trades?.length ?? "—" },
                    { label: "total volume", value: fmtQty(totalVol) },
                    { label: "buy / sell", value: `${buys.length} / ${sells.length}` },
                ].map((s) => (
                    <div key={s.label} className="px-5 py-4 bg-bg-surface">
                        <p className="text-[11px] mb-1 uppercase tracking-wider font-medium text-text-muted">
                            {s.label}
                        </p>
                        <p className="text-xl font-mono font-bold text-white tracking-tight">{s.value}</p>
                    </div>
                ))}
            </div>

            <Card title="recent trades" subtitle="showing aggregated across all presets" noPad>
                {loading && !trades ? (
                    <div className="flex items-center justify-center h-48 text-sm text-text-muted">fetching…</div>
                ) : !filteredTrades || filteredTrades.length === 0 ? (
                    <div className="flex flex-col items-center justify-center h-48 gap-3 text-sm text-text-muted">
                        <AlertCircle size={20} className="opacity-50" />
                        no trades found
                    </div>
                ) : (
                    <div className="max-h-[600px] overflow-y-auto">
                        <table className="w-full text-[11px] font-mono">
                            <thead>
                                <tr className="border-b border-border-strong bg-bg-surface text-text-muted uppercase tracking-wider sticky top-0 z-10">
                                    {["time", "exchange", "symbol", "price", "size", "side", "fee bps"].map((h) => (
                                        <th key={h} className="text-left px-5 py-3 font-medium">
                                            {h}
                                        </th>
                                    ))}
                                </tr>
                            </thead>
                            <tbody>
                                {filteredTrades.map((t, i) => {
                                    const isBuy = t.side === "BUY" || t.side === "buy";
                                    return (
                                        <tr
                                            key={i}
                                            className="border-b border-border-strong hover:bg-bg-hover transition-colors text-xs"
                                        >
                                            <td className="px-5 py-2.5 text-text-muted whitespace-nowrap">
                                                {fmtTs(t.timestamp)}
                                            </td>
                                            <td className="px-5 py-2.5 text-text-primary capitalize">
                                                {exchangeName(t)}
                                            </td>
                                            <td className="px-5 py-2.5 text-text-primary font-bold">{t.symbol}</td>
                                            <td
                                                className={`px-5 py-2.5 font-medium ${isBuy ? "text-accent-green" : "text-accent-red"}`}
                                            >
                                                {fmtPrice(t.price)}
                                            </td>
                                            <td className="px-5 py-2.5 text-text-primary">{fmtQty(t.size)}</td>
                                            <td className="px-5 py-2.5">
                                                <Badge
                                                    label={isBuy ? "BUY" : "SELL"}
                                                    variant={isBuy ? "green" : "red"}
                                                />
                                            </td>
                                            <td className="px-5 py-2.5 text-text-muted">{t.fee_rate_bps}</td>
                                        </tr>
                                    );
                                })}
                            </tbody>
                        </table>
                    </div>
                )}
            </Card>
        </div>
    );
}
