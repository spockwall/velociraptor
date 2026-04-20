import { useCallback } from "react";
import { RefreshCw, AlertCircle } from "lucide-react";
import { api, type OrderbookSnapshot, type BbaPayload } from "../lib/api";
import { fmtPrice, fmtQty, fmtTs } from "../lib/format";
import { usePolling } from "../lib/usePolling";
import Card from "../components/Card";
import { PRESETS } from "../components/ExchangeSymbolPicker";

// ── palette classes ──────────────────────────────────────────────────────────
const classMap = {
    bidText: "text-accent-green",
    bidBg: "bg-accent-green/10",
    askText: "text-accent-red",
    askBg: "bg-accent-red/10",
    dim: "text-text-muted",
    mid: "text-text-primary",
};

// ── depth bar ────────────────────────────────────────────────────────────────
function DepthBar({ side, levels }: { side: "bid" | "ask"; levels: [number, number][] }) {
    const maxQty = Math.max(...levels.map((l) => l[1]), 0.001);
    const isBid = side === "bid";
    return (
        <div>
            {levels.slice(0, 16).map(([px, qty], i) => {
                const pct = (qty / maxQty) * 100;
                return (
                    <div key={i} className="relative flex items-center h-[22px] text-[11px] font-mono">
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

// ── spread row ───────────────────────────────────────────────────────────────
function SpreadRow({ snap }: { snap: OrderbookSnapshot }) {
    const spread = snap.spread ?? (snap.best_ask && snap.best_bid ? snap.best_ask[0] - snap.best_bid[0] : null);
    return (
        <div className="flex items-center justify-between px-4 py-2 text-xs border-y border-border-strong font-mono bg-bg-surface">
            <span className={classMap.dim}>spread</span>
            <span className={`${classMap.mid} font-semibold tracking-wide`}>
                {spread != null ? spread.toFixed(6) : "—"}
            </span>
            <span className={classMap.dim}>
                wmid <span className="text-white ml-2">{fmtPrice(snap.wmid)}</span>
            </span>
        </div>
    );
}

// ── BBA panel ────────────────────────────────────────────────────────────────
function BbaPanel({ bba, snap }: { bba: BbaPayload | null; snap: OrderbookSnapshot | null }) {
    const source = bba ?? snap;
    const bid = source?.best_bid?.[0];
    const ask = source?.best_ask?.[0];
    const spread = source?.spread ?? (bid != null && ask != null ? ask - bid : null);

    return (
        <div className="grid grid-cols-3 gap-px bg-border-strong">
            {[
                {
                    label: "Best Bid",
                    value: bid != null ? fmtPrice(bid) : "—",
                    colorClass: classMap.bidText,
                    qty: source?.best_bid?.[1],
                },
                {
                    label: "Spread",
                    value: spread != null ? spread.toFixed(6) : "—",
                    colorClass: classMap.mid,
                    qty: null,
                },
                {
                    label: "Best Ask",
                    value: ask != null ? fmtPrice(ask) : "—",
                    colorClass: classMap.askText,
                    qty: source?.best_ask?.[1],
                },
            ].map((s) => (
                <div
                    key={s.label}
                    className="flex flex-col px-5 py-4 bg-bg-surface hover:bg-bg-hover transition-colors"
                >
                    <span className={`text-xs mb-2 tracking-wider uppercase font-medium ${classMap.dim}`}>
                        {s.label}
                    </span>
                    <span className={`text-xl font-mono font-bold leading-none ${s.colorClass}`}>{s.value}</span>
                    {s.qty != null && (
                        <span className={`text-[11px] mt-1.5 font-mono ${classMap.dim} opacity-70`}>
                            {fmtQty(s.qty)}
                        </span>
                    )}
                </div>
            ))}
        </div>
    );
}

// ── panel ───────────────────────────────────────────────────────────────────
function OrderbookPanel({ exchange, symbol }: { exchange: string; symbol: string }) {
    const snapFetcher = useCallback(() => api.orderbook(exchange, symbol), [exchange, symbol]);
    const bbaFetcher = useCallback(() => api.bba(exchange, symbol), [exchange, symbol]);

    const { data: snap, error, loading, refetch } = usePolling<OrderbookSnapshot>(snapFetcher, 800);
    const { data: bba } = usePolling<BbaPayload>(bbaFetcher, 500);

    return (
        <Card title={`${exchange} / ${symbol}`} subtitle="live depth · 800ms" noPad>
            <div className="flex items-center justify-between px-4 py-2 border-b border-border-strong bg-bg-surface/50">
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
                <div className="m-3 flex items-center gap-2 px-3 py-2 rounded-md border text-xs bg-accent-red/10 border-accent-red/20 text-accent-red shadow-sm">
                    <AlertCircle size={14} />
                    {error}
                </div>
            )}

            <BbaPanel bba={bba} snap={snap} />

            {snap ? (
                <>
                    <div
                        className={`grid grid-cols-2 px-4 pt-3 pb-2 text-[10px] font-mono font-medium ${classMap.dim} uppercase tracking-wider border-t border-border-strong`}
                    >
                        <div className="flex justify-between pr-2 text-right">
                            <span className="w-16 text-right">QTY</span>
                            <span>BID</span>
                        </div>
                        <div className="flex justify-between pl-2">
                            <span>ASK</span>
                            <span className="w-16">QTY</span>
                        </div>
                    </div>
                    <div className="grid grid-cols-2 gap-px px-2 pb-2 bg-border-strong">
                        <div className="bg-bg-surface p-0.5 rounded-l-md overflow-hidden">
                            <DepthBar side="bid" levels={snap.bids} />
                        </div>
                        <div className="bg-bg-surface p-0.5 rounded-r-md overflow-hidden">
                            <DepthBar side="ask" levels={snap.asks} />
                        </div>
                    </div>
                    <SpreadRow snap={snap} />
                </>
            ) : (
                <div className={`flex items-center justify-center h-40 text-sm ${classMap.dim}`}>
                    {loading ? "fetching…" : "no data"}
                </div>
            )}
        </Card>
    );
}

// ── page ──────────────────────────────────────────────────────────────────────
export default function OrderbookPage() {
    return (
        <div className="p-4 w-full max-w-[1600px] mx-auto">
            <div className="grid grid-cols-1 lg:grid-cols-2 2xl:grid-cols-3 gap-6">
                {PRESETS.map((p) => (
                    <OrderbookPanel key={`${p.exchange}-${p.symbol}`} exchange={p.exchange} symbol={p.symbol} />
                ))}
            </div>
        </div>
    );
}
