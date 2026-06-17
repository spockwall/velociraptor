// Per-tab tables + shared bits for the Polymarket Explorer page.
// Styled to match the account sub-pages (ROW_TEXT/HEADER_TEXT, Badge, Card).

import {
    Area,
    AreaChart,
    CartesianGrid,
    ReferenceLine,
    ResponsiveContainer,
    Tooltip,
    XAxis,
    YAxis,
} from "recharts";
import Badge from "../../components/Badge";
import { C } from "../../lib/colors";
import { fmtPrice, fmtQty } from "../../lib/format";
import type {
    PmActivity,
    PmClosedPosition,
    PmHolderGroup,
    PmLeaderboardEntry,
    PmMarketMetrics,
    PmPnlPoint,
    PmPosition,
    PmTrade,
} from "../../lib/api";

export const ROW_TEXT = "text-[10px]";
export const HEADER_TEXT = "text-[10px]";

// ── helpers ──────────────────────────────────────────────────────────────────

/** data-api timestamps are unix *seconds*. */
export function fmtTimeSecs(secs: number): string {
    if (!secs) return "—";
    const d = new Date(secs * 1000);
    const p = (n: number) => String(n).padStart(2, "0");
    return (
        `${d.getUTCFullYear()}/${p(d.getUTCMonth() + 1)}/${p(d.getUTCDate())}` +
        `-${p(d.getUTCHours())}:${p(d.getUTCMinutes())}:${p(d.getUTCSeconds())}`
    );
}

export function fmtUsd(v: number | null | undefined): string {
    if (v == null) return "—";
    return v.toLocaleString("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 0 });
}

export function shortHash(h: string, head = 6, tail = 4): string {
    if (!h) return "—";
    return h.length <= head + tail + 2 ? h : `${h.slice(0, head)}…${h.slice(-tail)}`;
}

function pnlClass(v: number): string {
    return v > 0 ? "text-accent-green" : v < 0 ? "text-accent-red" : "text-text-muted";
}

/** Color a Polymarket activity type / outcome consistently. */
function typeVariant(t: string): "green" | "red" | "yellow" | "gray" {
    switch (t.toUpperCase()) {
        case "TRADE":
            return "green";
        case "REDEEM":
        case "REWARD":
        case "MAKER_REBATE":
        case "YIELD":
            return "yellow";
        case "SPLIT":
        case "MERGE":
        case "CONVERSION":
            return "gray";
        default:
            return "gray";
    }
}

function outcomeVariant(o: string): "green" | "red" | "gray" {
    const s = o.toLowerCase();
    if (s === "yes" || s === "up") return "green";
    if (s === "no" || s === "down") return "red";
    return "gray";
}

export function EmptyState({
    loading,
    error,
    label,
}: {
    loading: boolean;
    error: string | null;
    label: string;
}) {
    if (loading) return <p className={`${ROW_TEXT} py-10 text-center text-text-muted`}>loading…</p>;
    if (error)
        return (
            <p className={`${ROW_TEXT} py-10 text-center text-text-muted`}>
                <span className="text-red-400">error:</span> {error}
            </p>
        );
    return <p className={`${ROW_TEXT} py-10 text-center text-text-muted`}>{label}</p>;
}

const TH = "py-1.5 pr-3 font-medium whitespace-nowrap";
const TD = "py-1 pr-3 whitespace-nowrap";

function Table({ head, children }: { head: React.ReactNode; children: React.ReactNode }) {
    return (
        <div className="overflow-x-auto">
            <table className={`w-full table-auto ${ROW_TEXT} font-mono`}>
                <thead>
                    <tr className={`text-text-muted text-left ${HEADER_TEXT}`}>{head}</tr>
                </thead>
                <tbody>{children}</tbody>
            </table>
        </div>
    );
}

// ── Positions ────────────────────────────────────────────────────────────────

export function PositionsTable({
    rows,
    loading,
    error,
    onPickMarket,
    emptyLabel = "no positions",
}: {
    rows: PmPosition[];
    loading: boolean;
    error: string | null;
    onPickMarket: (conditionId: string) => void;
    emptyLabel?: string;
}) {
    if (rows.length === 0) return <EmptyState loading={loading} error={error} label={emptyLabel} />;
    return (
        <Table
            head={
                <>
                    <th className={TH}>market</th>
                    <th className={TH}>outcome</th>
                    <th className={`${TH} text-right`}>size</th>
                    <th className={`${TH} text-right`}>avg</th>
                    <th className={`${TH} text-right`}>cur</th>
                    <th className={`${TH} text-right`}>value</th>
                    <th className={`${TH} text-right`}>cash pnl</th>
                    <th className={`${TH} text-right`}>pnl %</th>
                    <th className={TH}></th>
                </>
            }
        >
            {rows.map((p, i) => (
                <tr key={`${p.asset}-${i}`} className="border-t border-border-strong">
                    <td className={`${TD} text-text-muted truncate max-w-[34ch]`} title={p.title}>
                        {p.title}
                    </td>
                    <td className={TD}>
                        <Badge label={p.outcome || "—"} variant={outcomeVariant(p.outcome)} />
                    </td>
                    <td className={`${TD} text-right text-white`}>{fmtQty(p.size)}</td>
                    <td className={`${TD} text-right text-text-muted`}>{fmtPrice(p.avgPrice)}</td>
                    <td className={`${TD} text-right text-white`}>{fmtPrice(p.curPrice)}</td>
                    <td className={`${TD} text-right text-white`}>{fmtUsd(p.currentValue)}</td>
                    <td className={`${TD} text-right ${pnlClass(p.cashPnl)}`}>{fmtUsd(p.cashPnl)}</td>
                    <td className={`${TD} text-right ${pnlClass(p.percentPnl)}`}>
                        {p.percentPnl?.toFixed(2)}%
                    </td>
                    <td className={TD}>
                        <button
                            onClick={() => onPickMarket(p.conditionId)}
                            className="text-[9px] px-1.5 py-0.5 rounded border border-border-strong text-text-muted hover:text-white hover:bg-bg-hover transition-colors"
                            title="View holders for this market"
                        >
                            holders
                        </button>
                    </td>
                </tr>
            ))}
        </Table>
    );
}

// ── Closed positions (data-api /closed-positions) ────────────────────────────

export function ClosedPositionsTable({
    rows,
    loading,
    error,
    onPickMarket,
}: {
    rows: PmClosedPosition[];
    loading: boolean;
    error: string | null;
    onPickMarket: (conditionId: string) => void;
}) {
    if (rows.length === 0)
        return <EmptyState loading={loading} error={error} label="no closed positions" />;
    return (
        <Table
            head={
                <>
                    <th className={TH}>resolved</th>
                    <th className={TH}>market</th>
                    <th className={TH}>outcome</th>
                    <th className={`${TH} text-right`}>avg</th>
                    <th className={`${TH} text-right`}>bought</th>
                    <th className={`${TH} text-right`}>realized pnl</th>
                    <th className={TH}></th>
                </>
            }
        >
            {rows.map((p, i) => (
                <tr key={`${p.asset}-${i}`} className="border-t border-border-strong">
                    <td className={`${TD} text-text-muted`}>{fmtTimeSecs(p.timestamp)}</td>
                    <td className={`${TD} text-text-muted truncate max-w-[34ch]`} title={p.title}>
                        {p.title}
                    </td>
                    <td className={TD}>
                        <Badge label={p.outcome || "—"} variant={outcomeVariant(p.outcome)} />
                    </td>
                    <td className={`${TD} text-right text-text-muted`}>{fmtPrice(p.avgPrice)}</td>
                    <td className={`${TD} text-right text-white`}>{fmtUsd(p.totalBought)}</td>
                    <td className={`${TD} text-right ${pnlClass(p.realizedPnl)}`}>
                        {fmtUsd(p.realizedPnl)}
                    </td>
                    <td className={TD}>
                        <button
                            onClick={() => onPickMarket(p.conditionId)}
                            className="text-[9px] px-1.5 py-0.5 rounded border border-border-strong text-text-muted hover:text-white hover:bg-bg-hover transition-colors"
                            title="View holders for this market"
                        >
                            holders
                        </button>
                    </td>
                </tr>
            ))}
        </Table>
    );
}

// ── Trades (data-api /trades) ────────────────────────────────────────────────

export function TradesTable({
    rows,
    loading,
    error,
}: {
    rows: PmTrade[];
    loading: boolean;
    error: string | null;
}) {
    if (rows.length === 0) return <EmptyState loading={loading} error={error} label="no trades" />;
    return (
        <Table
            head={
                <>
                    <th className={TH}>time</th>
                    <th className={TH}>side</th>
                    <th className={TH}>outcome</th>
                    <th className={`${TH} text-right`}>price</th>
                    <th className={`${TH} text-right`}>size</th>
                    <th className={`${TH} text-right`}>value</th>
                    <th className={TH}>market</th>
                    <th className={TH}>tx</th>
                </>
            }
        >
            {rows.map((t, i) => (
                <tr key={`${t.transactionHash}-${i}`} className="border-t border-border-strong">
                    <td className={`${TD} text-text-muted`}>{fmtTimeSecs(t.timestamp)}</td>
                    <td className={TD}>
                        <Badge
                            label={t.side}
                            variant={t.side.toUpperCase() === "BUY" ? "green" : "red"}
                        />
                    </td>
                    <td className={TD}>
                        <Badge label={t.outcome || "—"} variant={outcomeVariant(t.outcome)} />
                    </td>
                    <td className={`${TD} text-right text-text-muted`}>{fmtPrice(t.price)}</td>
                    <td className={`${TD} text-right text-white`}>{fmtQty(t.size)}</td>
                    <td className={`${TD} text-right text-white`}>{fmtUsd(t.price * t.size)}</td>
                    <td className={`${TD} text-text-muted truncate max-w-[30ch]`} title={t.title}>
                        {t.title || "—"}
                    </td>
                    <td className={TD}>
                        {t.transactionHash ? (
                            <a
                                href={`https://polygonscan.com/tx/${t.transactionHash}`}
                                target="_blank"
                                rel="noreferrer"
                                className="text-text-muted hover:text-white underline"
                            >
                                {shortHash(t.transactionHash)}
                            </a>
                        ) : (
                            "—"
                        )}
                    </td>
                </tr>
            ))}
        </Table>
    );
}

// ── Activity / Trades ─────────────────────────────────────────────────────────

export function ActivityTable({
    rows,
    loading,
    error,
    showType,
}: {
    rows: PmActivity[];
    loading: boolean;
    error: string | null;
    showType: boolean;
}) {
    if (rows.length === 0)
        return <EmptyState loading={loading} error={error} label={showType ? "no activity" : "no trades"} />;
    return (
        <Table
            head={
                <>
                    <th className={TH}>time</th>
                    {showType && <th className={TH}>type</th>}
                    <th className={TH}>side</th>
                    <th className={TH}>outcome</th>
                    <th className={`${TH} text-right`}>price</th>
                    <th className={`${TH} text-right`}>size</th>
                    <th className={`${TH} text-right`}>usdc</th>
                    <th className={TH}>market</th>
                    <th className={TH}>tx</th>
                </>
            }
        >
            {rows.map((a, i) => (
                <tr key={`${a.transactionHash}-${i}`} className="border-t border-border-strong">
                    <td className={`${TD} text-text-muted`}>{fmtTimeSecs(a.timestamp)}</td>
                    {showType && (
                        <td className={TD}>
                            <Badge label={a.type} variant={typeVariant(a.type)} />
                        </td>
                    )}
                    <td className={TD}>
                        {a.side ? (
                            <Badge label={a.side} variant={a.side.toUpperCase() === "BUY" ? "green" : "red"} />
                        ) : (
                            <span className="text-text-muted">—</span>
                        )}
                    </td>
                    <td className={TD}>
                        {a.outcome ? (
                            <Badge label={a.outcome} variant={outcomeVariant(a.outcome)} />
                        ) : (
                            <span className="text-text-muted">—</span>
                        )}
                    </td>
                    <td className={`${TD} text-right text-text-muted`}>{a.price ? fmtPrice(a.price) : "—"}</td>
                    <td className={`${TD} text-right text-white`}>{fmtQty(a.size)}</td>
                    <td className={`${TD} text-right text-white`}>{fmtUsd(a.usdcSize)}</td>
                    <td className={`${TD} text-text-muted truncate max-w-[30ch]`} title={a.title}>
                        {a.title || "—"}
                    </td>
                    <td className={TD}>
                        {a.transactionHash ? (
                            <a
                                href={`https://polygonscan.com/tx/${a.transactionHash}`}
                                target="_blank"
                                rel="noreferrer"
                                className="text-text-muted hover:text-white underline"
                            >
                                {shortHash(a.transactionHash)}
                            </a>
                        ) : (
                            "—"
                        )}
                    </td>
                </tr>
            ))}
        </Table>
    );
}

// ── Holders ──────────────────────────────────────────────────────────────────

export function HoldersTable({
    groups,
    loading,
    error,
    onPickWallet,
}: {
    groups: PmHolderGroup[];
    loading: boolean;
    error: string | null;
    onPickWallet: (wallet: string) => void;
}) {
    const hasAny = groups.some((g) => g.holders?.length);
    if (!hasAny) return <EmptyState loading={loading} error={error} label="no holders" />;
    return (
        <div className="space-y-5">
            {groups.map((g, gi) => (
                <div key={`${g.token}-${gi}`}>
                    <p className={`${HEADER_TEXT} font-mono text-text-muted mb-1`}>
                        token {shortHash(g.token, 8, 6)}
                    </p>
                    <Table
                        head={
                            <>
                                <th className={`${TH} text-right`}>#</th>
                                <th className={TH}>holder</th>
                                <th className={`${TH} text-right`}>amount</th>
                                <th className={TH}>wallet</th>
                            </>
                        }
                    >
                        {g.holders.map((h, i) => (
                            <tr key={`${h.proxyWallet}-${i}`} className="border-t border-border-strong">
                                <td className={`${TD} text-right text-text-muted`}>{i + 1}</td>
                                <td className={`${TD} text-white`}>{h.name || h.pseudonym || "—"}</td>
                                <td className={`${TD} text-right text-white`}>{fmtQty(h.amount)}</td>
                                <td className={TD}>
                                    <button
                                        onClick={() => onPickWallet(h.proxyWallet)}
                                        className="text-text-muted hover:text-white underline"
                                    >
                                        {shortHash(h.proxyWallet)}
                                    </button>
                                </td>
                            </tr>
                        ))}
                    </Table>
                </div>
            ))}
        </div>
    );
}

// ── Leaderboard ──────────────────────────────────────────────────────────────

export function LeaderboardTable({
    rows,
    loading,
    error,
    metric,
    onPickWallet,
}: {
    rows: PmLeaderboardEntry[];
    loading: boolean;
    error: string | null;
    metric: "volume" | "profit";
    onPickWallet: (wallet: string) => void;
}) {
    if (rows.length === 0) return <EmptyState loading={loading} error={error} label="no entries" />;
    return (
        <Table
            head={
                <>
                    <th className={`${TH} text-right`}>#</th>
                    <th className={TH}>trader</th>
                    <th className={`${TH} text-right`}>{metric}</th>
                    <th className={TH}>wallet</th>
                </>
            }
        >
            {rows.map((e, i) => (
                <tr key={`${e.proxyWallet}-${i}`} className="border-t border-border-strong">
                    <td className={`${TD} text-right text-text-muted`}>{i + 1}</td>
                    <td className={`${TD} text-white`}>{e.name || e.pseudonym || "—"}</td>
                    <td className={`${TD} text-right text-white`}>{fmtUsd(e.amount)}</td>
                    <td className={TD}>
                        <button
                            onClick={() => onPickWallet(e.proxyWallet)}
                            className="text-text-muted hover:text-white underline"
                        >
                            {shortHash(e.proxyWallet)}
                        </button>
                    </td>
                </tr>
            ))}
        </Table>
    );
}

// ── Market metrics (the "open interest" substitute) ──────────────────────────

export function MarketMetrics({
    data,
    loading,
    error,
}: {
    data: PmMarketMetrics | null;
    loading: boolean;
    error: string | null;
}) {
    if (!data) return <EmptyState loading={loading} error={error} label="enter a market slug" />;
    const stats: [string, string][] = [
        ["liquidity", fmtUsd(data.liquidity)],
        ["volume (total)", fmtUsd(data.volume)],
        ["volume 24h", fmtUsd(data.volume24hr)],
        ["volume 1wk", fmtUsd(data.volume1wk)],
        ["volume 1mo", fmtUsd(data.volume1mo)],
        ["best bid", data.bestBid != null ? fmtPrice(data.bestBid) : "—"],
        ["best ask", data.bestAsk != null ? fmtPrice(data.bestAsk) : "—"],
        ["last trade", data.lastTradePrice != null ? fmtPrice(data.lastTradePrice) : "—"],
    ];
    return (
        <div>
            {data.question && (
                <p className="text-[11px] text-white mb-3 font-medium">{data.question}</p>
            )}
            <div className="grid grid-cols-2 sm:grid-cols-4 gap-px bg-border-strong rounded overflow-hidden">
                {stats.map(([label, val]) => (
                    <div key={label} className="bg-bg-card p-3">
                        <p className="text-[9px] uppercase tracking-wide text-text-muted">{label}</p>
                        <p className="text-[13px] font-mono text-white mt-1">{val}</p>
                    </div>
                ))}
            </div>
            <p className="text-[9px] text-text-muted mt-3 font-mono">
                note: Polymarket exposes no open-interest field — liquidity / volume shown instead.
            </p>
        </div>
    );
}

// ── P&L chart ────────────────────────────────────────────────────────────────

function fmtPnlAxisDate(t: number): string {
    const d = new Date(t * 1000);
    const p = (n: number) => String(n).padStart(2, "0");
    return `${p(d.getUTCMonth() + 1)}/${p(d.getUTCDate())}`;
}

export function PnlChart({
    points,
    loading,
    error,
}: {
    points: PmPnlPoint[];
    loading: boolean;
    error: string | null;
}) {
    if (points.length < 2) {
        return (
            <EmptyState
                loading={loading}
                error={error}
                label="not enough P&L history to chart"
            />
        );
    }
    // Rebase to the window start so the curve shows P&L *within the selected
    // range* (starts at 0). The headline is the change across the window.
    const base = points[0].p;
    const series = points.map((pt) => ({ t: pt.t, p: pt.p - base }));
    const total = series[series.length - 1].p;
    const positive = total >= 0;
    const stroke = positive ? C.green : C.red;

    return (
        <div>
            <div className="mb-3">
                <p className="text-[9px] uppercase tracking-wide text-text-muted">P&amp;L over range</p>
                <p
                    className="text-[20px] font-mono mt-0.5"
                    style={{ color: stroke }}
                >
                    {positive ? "+" : ""}
                    {fmtUsd(total)}
                </p>
            </div>
            <div className="h-64">
                <ResponsiveContainer width="100%" height="100%">
                    <AreaChart data={series} margin={{ top: 8, right: 12, bottom: 0, left: -8 }}>
                        <defs>
                            <linearGradient id="pnlFill" x1="0" y1="0" x2="0" y2="1">
                                <stop offset="0%" stopColor={stroke} stopOpacity={0.35} />
                                <stop offset="100%" stopColor={stroke} stopOpacity={0} />
                            </linearGradient>
                        </defs>
                        <CartesianGrid stroke={C.borderCard} vertical={false} />
                        <XAxis
                            dataKey="t"
                            tickFormatter={fmtPnlAxisDate}
                            tick={{ fill: C.textSubtle, fontSize: 10 }}
                            stroke={C.borderStrong}
                            minTickGap={48}
                        />
                        <YAxis
                            tickFormatter={(v) => fmtUsd(Number(v))}
                            tick={{ fill: C.textSubtle, fontSize: 10 }}
                            stroke={C.borderStrong}
                            width={56}
                        />
                        <ReferenceLine y={0} stroke={C.borderStrong} />
                        <Tooltip
                            contentStyle={{
                                background: C.bgCard,
                                border: `1px solid ${C.borderStrong}`,
                                borderRadius: 6,
                                fontSize: 12,
                            }}
                            labelStyle={{ color: C.textDim }}
                            labelFormatter={(t) => {
                                const d = new Date(Number(t) * 1000);
                                return d.toISOString().slice(0, 10);
                            }}
                            formatter={(v) => [fmtUsd(Number(v)), "P&L"]}
                        />
                        <Area
                            type="monotone"
                            dataKey="p"
                            stroke={stroke}
                            strokeWidth={1.5}
                            fill="url(#pnlFill)"
                            dot={false}
                            isAnimationActive={false}
                        />
                    </AreaChart>
                </ResponsiveContainer>
            </div>
        </div>
    );
}
