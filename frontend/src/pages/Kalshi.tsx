import { useCallback } from "react";
import { RefreshCw, AlertCircle } from "lucide-react";
import { api, type OrderbookSnapshot, type BbaPayload, type KalshiMarket } from "../lib/api";
import { fmtPrice, fmtQty, fmtTs } from "../lib/format";
import { usePolling } from "../lib/usePolling";
import Card from "../components/Card";

const classMap = {
    bidText: "text-accent-green",
    bidBg: "bg-accent-green/10",
    askText: "text-accent-red",
    askBg: "bg-accent-red/10",
    dim: "text-text-muted",
    mid: "text-text-primary",
};

function DepthBar({ side, levels }: { side: "bid" | "ask"; levels: [number, number][] }) {
    const maxQty = Math.max(...levels.map((l) => l[1]), 0.001);
    const isBid = side === "bid";
    return (
        <div>
            {levels.slice(0, 12).map(([px, qty], i) => {
                const pct = (qty / maxQty) * 100;
                return (
                    <div key={i} className="relative flex items-center h-[20px] text-[11px] font-mono">
                        <div
                            className={`absolute inset-y-0 ${isBid ? classMap.bidBg : classMap.askBg}`}
                            style={{ width: `${pct}%`, [isBid ? "right" : "left"]: 0 }}
                        />
                        {isBid ? (
                            <>
                                <span className={`relative z-10 flex-1 text-right pr-3 ${classMap.bidText}`}>
                                    {fmtPrice(px)}
                                </span>
                                <span className={`relative z-10 w-20 text-right pr-2 ${classMap.dim}`}>
                                    {fmtQty(qty)}
                                </span>
                            </>
                        ) : (
                            <>
                                <span className={`relative z-10 w-20 pl-2 ${classMap.dim}`}>{fmtQty(qty)}</span>
                                <span className={`relative z-10 flex-1 pl-3 ${classMap.askText}`}>{fmtPrice(px)}</span>
                            </>
                        )}
                    </div>
                );
            })}
        </div>
    );
}

function MarketPanel({ market }: { market: KalshiMarket }) {
    const snapFetcher = useCallback(() => api.orderbook("kalshi", market.ticker), [market.ticker]);
    const bbaFetcher = useCallback(() => api.bba("kalshi", market.ticker), [market.ticker]);

    const { data: snap, error, loading, refetch } = usePolling<OrderbookSnapshot>(snapFetcher, 1000);
    const { data: bba } = usePolling<BbaPayload>(bbaFetcher, 600);

    const source = bba ?? snap;
    const bid = source?.best_bid?.[0];
    const ask = source?.best_ask?.[0];
    const spread = source?.spread ?? (bid != null && ask != null ? ask - bid : null);

    const closeUnix = market.window_start + market.interval_secs;
    const closeLabel = market.interval_secs > 0 ? new Date(closeUnix * 1000).toISOString().slice(11, 16) + "Z" : "static";

    return (
        <Card title={market.title} subtitle={`${market.series} · closes ${closeLabel}`} noPad>
            <div className="flex items-center justify-between px-4 py-2 border-b border-border-strong bg-bg-surface/50">
                <span className={`text-[10px] font-mono uppercase tracking-wider ${classMap.dim}`}>
                    {market.series}
                </span>
                <span className="text-[10px] font-mono text-text-muted opacity-80">
                    {snap ? `seq ${snap.sequence} · ${fmtTs(snap.timestamp)}` : loading ? "loading…" : "waiting"}
                </span>
                <button
                    onClick={refetch}
                    className="p-1 rounded flex items-center justify-center text-text-muted hover:text-white hover:bg-bg-hover transition-colors cursor-pointer"
                >
                    <RefreshCw size={12} />
                </button>
            </div>

            {error && (
                <div className="m-3 flex items-center gap-2 px-3 py-2 rounded-md border text-xs bg-accent-red/10 border-accent-red/20 text-accent-red">
                    <AlertCircle size={14} />
                    {error}
                </div>
            )}

            <div className="grid grid-cols-3 gap-px bg-border-strong">
                {[
                    { label: "Bid", value: bid != null ? fmtPrice(bid) : "—", colorClass: classMap.bidText },
                    {
                        label: "Spread",
                        value: spread != null ? spread.toFixed(4) : "—",
                        colorClass: classMap.mid,
                    },
                    { label: "Ask", value: ask != null ? fmtPrice(ask) : "—", colorClass: classMap.askText },
                ].map((s) => (
                    <div key={s.label} className="flex flex-col px-3 py-2 bg-bg-surface">
                        <span className={`text-[10px] mb-1 tracking-wider uppercase ${classMap.dim}`}>{s.label}</span>
                        <span className={`text-sm font-mono font-bold leading-none ${s.colorClass}`}>{s.value}</span>
                    </div>
                ))}
            </div>

            {snap ? (
                <div className="grid grid-cols-2 gap-px px-2 pb-2 pt-2 bg-border-strong">
                    <div className="bg-bg-surface p-0.5 rounded-l-md overflow-hidden">
                        <DepthBar side="bid" levels={snap.bids} />
                    </div>
                    <div className="bg-bg-surface p-0.5 rounded-r-md overflow-hidden">
                        <DepthBar side="ask" levels={snap.asks} />
                    </div>
                </div>
            ) : (
                <div className={`flex items-center justify-center h-32 text-sm ${classMap.dim}`}>
                    {loading ? "fetching…" : "no data"}
                </div>
            )}
        </Card>
    );
}

export default function KalshiPage() {
    const fetcher = useCallback(() => api.kalshiMarkets(), []);
    const { data: markets, error, loading } = usePolling<KalshiMarket[]>(fetcher, 5000);

    return (
        <div className="p-4 w-full max-w-[1600px] mx-auto">
            <div className="flex items-center justify-between mb-4">
                <h1 className="text-sm font-mono text-text-primary">
                    Kalshi · live windows
                    {markets && (
                        <span className="ml-3 text-text-muted">
                            ({markets.length} ticker{markets.length === 1 ? "" : "s"})
                        </span>
                    )}
                </h1>
            </div>

            {error && (
                <div className="flex items-center gap-2 px-3 py-2 rounded-md border text-xs bg-accent-red/10 border-accent-red/20 text-accent-red mb-4">
                    <AlertCircle size={14} />
                    {error}
                </div>
            )}

            {!markets && loading && (
                <div className="text-center text-sm text-text-muted py-12">discovering markets…</div>
            )}

            {markets && markets.length === 0 && (
                <div className="text-center text-sm text-text-muted py-12">
                    no active Kalshi windows — start orderbook_server with kalshi enabled
                </div>
            )}

            {markets && markets.length > 0 && (
                <div className="grid grid-cols-1 lg:grid-cols-2 2xl:grid-cols-3 gap-6">
                    {markets.map((m) => (
                        <MarketPanel key={m.ticker} market={m} />
                    ))}
                </div>
            )}
        </div>
    );
}
