// Polymarket Explorer — inspect ANY Polymarket account (by wallet address, or
// best-effort username) plus market holder / leaderboard data. All public,
// proxied through the backend `/api/pm/*` routes.

import { useCallback, useEffect, useMemo, useState } from "react";
import { RefreshCw, Search } from "lucide-react";
import Card from "../../components/Card";
import { api } from "../../lib/api";
import type {
    PmActivity,
    PmClosedPosition,
    PmHolderGroup,
    PmLeaderboardEntry,
    PmMarketMetrics,
    PmPnlPoint,
    PmPosition,
    PmTrade,
    PmValue,
} from "../../lib/api";
import { usePolling } from "../../lib/usePolling";
import {
    ActivityTable,
    ClosedPositionsTable,
    EmptyState,
    HoldersTable,
    LeaderboardTable,
    MarketMetrics,
    PnlChart,
    PositionsTable,
    TradesTable,
    fmtUsd,
    shortHash,
} from "./panels";

type Tab = "positions" | "pnl" | "trades" | "activity" | "holders" | "markets" | "leaderboard";

const TABS: { id: Tab; label: string; needsWallet: boolean }[] = [
    { id: "positions", label: "Positions", needsWallet: true },
    { id: "pnl", label: "Cumulative P&L", needsWallet: true },
    { id: "trades", label: "Trades", needsWallet: true },
    { id: "activity", label: "Activity", needsWallet: true },
    { id: "holders", label: "Holders", needsWallet: false },
    { id: "markets", label: "Markets", needsWallet: false },
    { id: "leaderboard", label: "Leaderboard", needsWallet: false },
];

// localStorage key for the last looked-up address/username.
const LS_KEY = "pmExplorer.lastInput";

type PnlInterval = "1d" | "1w" | "1m" | "all";

/** Short windows get hourly points; longer ones daily. */
function pnlFidelity(interval: PnlInterval): "1h" | "1d" {
    return interval === "1d" || interval === "1w" ? "1h" : "1d";
}

/**
 * P&L *over the selected range* = the change in cumulative P&L across the
 * window: last point − first point. For `all`, the first point is ~0 so this
 * equals the absolute cumulative total. Matches the site's per-range framing.
 */
function rangePnl(points: PmPnlPoint[] | null | undefined): number | null {
    if (!points || points.length === 0) return null;
    return points[points.length - 1].p - points[0].p;
}

export default function Explorer() {
    // The search box holds whatever the user typed (address or username); once
    // resolved, `wallet` is the canonical 0x address the data tabs query. Seed
    // it from localStorage so the last lookup survives reloads.
    const [input, setInput] = useState(() => {
        try {
            return localStorage.getItem(LS_KEY) ?? "";
        } catch {
            return "";
        }
    });
    const [wallet, setWallet] = useState<string>("");
    const [resolveErr, setResolveErr] = useState<string | null>(null);
    // Human label for the resolved wallet, e.g. "plswin123 · proxy of 0x192c…".
    const [resolvedLabel, setResolvedLabel] = useState<string | null>(null);
    const [tab, setTab] = useState<Tab>("positions");

    // Holders + Markets take their own per-tab inputs (a conditionId / slug).
    const [conditionId, setConditionId] = useState("");
    const [slug, setSlug] = useState("");

    // Leaderboard controls.
    const [lbMetric, setLbMetric] = useState<"volume" | "profit">("volume");
    const [lbWindow, setLbWindow] = useState<"all" | "7d" | "30d">("all");

    // P&L time range — shared between the top summary box and the P&L chart so
    // the headline P&L number is the change over the *selected* range.
    const [pnlRange, setPnlRange] = useState<PnlInterval>("all");

    // Shared refresh signal: every panel folds `refreshKey` into its fetcher
    // deps, so bumping it re-fetches all visible panels at once. It ticks
    // automatically every 10s and on the manual Refresh button.
    const [refreshKey, setRefreshKey] = useState(0);
    const refresh = useCallback(() => setRefreshKey((k) => k + 1), []);
    useEffect(() => {
        const id = window.setInterval(refresh, 10_000);
        return () => window.clearInterval(id);
    }, [refresh]);

    async function lookup(raw: string) {
        const id = raw.trim();
        if (!id) return;
        setResolveErr(null);
        try {
            const r = await api.pmResolve(id);
            setWallet(r.wallet);
            const who = r.name || r.pseudonym;
            const label =
                r.source === "proxy"
                    ? `${who ? `${who} · ` : ""}proxy of ${shortHash(r.input ?? id, 6, 4)}`
                    : r.source === "leaderboard"
                      ? `${who ?? id} (matched on leaderboard)`
                      : null;
            setResolvedLabel(label);
            // Cache the (successful) input so it's restored on next visit.
            try {
                localStorage.setItem(LS_KEY, id);
            } catch {
                /* ignore quota / disabled storage */
            }
            refresh(); // force re-fetch even if the wallet is unchanged
            if (TABS.find((t) => t.id === tab)?.needsWallet === false) setTab("positions");
        } catch (e) {
            setResolveErr(e instanceof Error ? e.message : String(e));
            setWallet("");
            setResolvedLabel(null);
        }
    }

    // On first mount, auto-look-up the cached input so the page restores its
    // last account without a manual submit. Runs once.
    useEffect(() => {
        if (input.trim()) lookup(input);
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    // Pick a wallet from a row (leaderboard / holders) → load its account.
    const pickWallet = useCallback((w: string) => {
        setInput(w);
        setResolveErr(null);
        setResolvedLabel(null);
        setWallet(w.toLowerCase());
        setTab("positions");
        try {
            localStorage.setItem(LS_KEY, w);
        } catch {
            /* ignore */
        }
    }, []);

    const pickMarket = useCallback((cid: string) => {
        setConditionId(cid);
        setTab("holders");
    }, []);

    return (
        <div className="p-5 w-full max-w-6xl mx-auto space-y-5">
            {/* ── search header ── */}
            <div className="flex flex-col gap-3">
                <form
                    className="flex items-center gap-2"
                    onSubmit={(e) => {
                        e.preventDefault();
                        lookup(input);
                    }}
                >
                    <div className="relative flex-1 max-w-xl">
                        <Search
                            size={14}
                            className="absolute left-3 top-1/2 -translate-y-1/2 text-text-muted"
                        />
                        <input
                            type="text"
                            placeholder="0x wallet address  (or username, best-effort)…"
                            value={input}
                            onChange={(e) => setInput(e.target.value)}
                            className="w-full pl-9 pr-4 py-2 rounded-md border border-border-strong bg-bg-surface text-white placeholder:text-text-muted text-xs font-mono focus:outline-none focus:border-border-subtle shadow-sm transition-all"
                        />
                    </div>
                    <button
                        type="submit"
                        className="px-4 py-2 rounded-md text-xs font-medium border border-border-strong text-white bg-bg-hover hover:border-border-subtle transition-colors"
                    >
                        Look up
                    </button>
                    <button
                        type="button"
                        onClick={refresh}
                        title="Refresh now (auto-refreshes every 10s)"
                        className="flex items-center gap-1.5 px-3 py-2 rounded-md text-xs font-medium border border-border-strong text-text-muted hover:text-white hover:border-border-subtle transition-colors"
                    >
                        <RefreshCw size={13} />
                        Refresh
                    </button>
                </form>
                <HeaderStats
                    wallet={wallet}
                    resolveErr={resolveErr}
                    label={resolvedLabel}
                    refreshKey={refreshKey}
                />
            </div>

            {/* ── always-visible summary box: portfolio value + range P&L ── */}
            {wallet && (
                <ProfileSummary
                    wallet={wallet}
                    refreshKey={refreshKey}
                    range={pnlRange}
                    onRangeChange={setPnlRange}
                />
            )}

            {/* ── sub-tabs ── */}
            <div className="flex items-center gap-1 flex-wrap">
                {TABS.map((t) => (
                    <button
                        key={t.id}
                        onClick={() => setTab(t.id)}
                        className={`px-3 py-1 rounded-md text-[11px] font-medium border transition-colors ${
                            tab === t.id
                                ? "text-white bg-bg-hover border-border-strong"
                                : "text-text-muted border-transparent hover:text-text-primary hover:bg-bg-hover hover:border-border-subtle"
                        }`}
                    >
                        {t.label}
                    </button>
                ))}
            </div>

            {/* ── active panel ── */}
            {tab === "positions" && (
                <PositionsPanel wallet={wallet} onPickMarket={pickMarket} refreshKey={refreshKey} />
            )}
            {tab === "pnl" && (
                <PnlPanel
                    wallet={wallet}
                    refreshKey={refreshKey}
                    range={pnlRange}
                    onRangeChange={setPnlRange}
                />
            )}
            {tab === "trades" && <TradesPanel wallet={wallet} refreshKey={refreshKey} />}
            {tab === "activity" && <ActivityPanel wallet={wallet} refreshKey={refreshKey} />}
            {tab === "holders" && (
                <HoldersPanel
                    conditionId={conditionId}
                    setConditionId={setConditionId}
                    onPickWallet={pickWallet}
                    refreshKey={refreshKey}
                />
            )}
            {tab === "markets" && (
                <MarketsPanel slug={slug} setSlug={setSlug} refreshKey={refreshKey} />
            )}
            {tab === "leaderboard" && (
                <LeaderboardPanel
                    metric={lbMetric}
                    setMetric={setLbMetric}
                    window={lbWindow}
                    setWindow={setLbWindow}
                    onPickWallet={pickWallet}
                    refreshKey={refreshKey}
                />
            )}
        </div>
    );
}

// ── header stats (portfolio value) ───────────────────────────────────────────

function HeaderStats({
    wallet,
    resolveErr,
    label,
    refreshKey,
}: {
    wallet: string;
    resolveErr: string | null;
    label: string | null;
    refreshKey: number;
}) {
    void refreshKey; // accepted for prop symmetry; this row shows no polled data
    if (resolveErr) {
        return <p className="text-[11px] font-mono text-accent-red">{resolveErr}</p>;
    }
    if (!wallet) {
        return (
            <p className="text-[11px] font-mono text-text-muted">
                enter a wallet to inspect positions, trades & activity
            </p>
        );
    }
    return (
        <p className="text-[11px] font-mono text-text-muted">
            {label && (
                <>
                    <span className="text-white">{label}</span>
                    {"  ·  "}
                </>
            )}
            wallet: <span className="text-white">{shortHash(wallet, 6, 4)}</span>
        </p>
    );
}

// ── always-visible summary: current portfolio value + total P&L ──────────────

function ProfileSummary({
    wallet,
    refreshKey,
    range,
    onRangeChange,
}: {
    wallet: string;
    refreshKey: number;
    range: PnlInterval;
    onRangeChange: (r: PnlInterval) => void;
}) {
    const fidelity = pnlFidelity(range);
    const valueFetcher = useCallback(() => api.pmValue(wallet), [wallet, refreshKey]);
    const pnlFetcher = useCallback(
        () => api.pmPnl(wallet, range, fidelity),
        [wallet, range, fidelity, refreshKey],
    );
    const { data: value } = usePolling<PmValue>(valueFetcher, 10_000, !!wallet);
    const { data: pnl } = usePolling<PmPnlPoint[]>(pnlFetcher, 10_000, !!wallet);

    // P&L over the selected range (change across the window).
    const pnlVal = rangePnl(pnl);
    const pnlColor = pnlVal == null ? "text-white" : pnlVal >= 0 ? "text-accent-green" : "text-accent-red";
    const rangeLabel = range === "all" ? "all-time" : `past ${range}`;

    const seg = (active: boolean) =>
        `px-2 py-0.5 rounded text-[10px] font-medium border transition-colors ${
            active
                ? "text-white bg-bg-hover border-border-strong"
                : "text-text-muted border-transparent hover:text-text-primary hover:bg-bg-hover"
        }`;

    return (
        <div className="grid grid-cols-2 gap-px bg-border-strong rounded overflow-hidden">
            <div className="bg-bg-card p-4">
                <p className="text-[9px] uppercase tracking-wide text-text-muted">portfolio value</p>
                <p className="text-[22px] font-mono text-white mt-1">
                    {value ? fmtUsd(value.value) : "—"}
                </p>
                <p className="text-[9px] text-text-muted mt-1 font-mono">current holdings (no public history)</p>
            </div>
            <div className="bg-bg-card p-4">
                <div className="flex items-center justify-between">
                    <p className="text-[9px] uppercase tracking-wide text-text-muted">P&amp;L ({rangeLabel})</p>
                    <div className="flex gap-1">
                        {(["1d", "1w", "1m", "all"] as const).map((r) => (
                            <button key={r} onClick={() => onRangeChange(r)} className={seg(range === r)}>
                                {r}
                            </button>
                        ))}
                    </div>
                </div>
                <p className={`text-[22px] font-mono mt-1 ${pnlColor}`}>
                    {pnlVal == null ? "—" : `${pnlVal >= 0 ? "+" : ""}${fmtUsd(pnlVal)}`}
                </p>
                <p className="text-[9px] text-text-muted mt-1 font-mono">cumulative profit / loss over range</p>
            </div>
        </div>
    );
}

// ── wallet-scoped panels ─────────────────────────────────────────────────────

function PositionsPanel({
    wallet,
    onPickMarket,
    refreshKey,
}: {
    wallet: string;
    onPickMarket: (cid: string) => void;
    refreshKey: number;
}) {
    // Active (open) positions come from /positions; we keep only the still-open
    // ones (not redeemable). Closed positions use the dedicated
    // /closed-positions endpoint (carries realizedPnl + resolved timestamp).
    const posFetcher = useCallback(() => api.pmPositions(wallet), [wallet, refreshKey]);
    const closedFetcher = useCallback(() => api.pmClosedPositions(wallet, 100), [wallet, refreshKey]);
    const positions = usePolling<PmPosition[]>(posFetcher, 10_000, !!wallet);
    const closed = usePolling<PmClosedPosition[]>(closedFetcher, 10_000, !!wallet);

    const active = useMemo(
        () => (positions.data ?? []).filter((p) => !p.redeemable),
        [positions.data],
    );
    const closedRows = closed.data ?? [];

    if (!wallet)
        return (
            <Card title="positions">
                <EmptyState loading={false} error={null} label="enter a wallet address" />
            </Card>
        );
    return (
        <div className="space-y-5">
            <Card title={`active positions (${active.length})`} subtitle="open · data-api /positions">
                <PositionsTable
                    rows={active}
                    loading={positions.loading}
                    error={positions.error}
                    onPickMarket={onPickMarket}
                    emptyLabel="no active positions"
                />
            </Card>
            <Card
                title={`closed positions (${closedRows.length})`}
                subtitle="resolved · data-api /closed-positions"
            >
                <ClosedPositionsTable
                    rows={closedRows}
                    loading={closed.loading}
                    error={closed.error}
                    onPickMarket={onPickMarket}
                />
            </Card>
        </div>
    );
}

function TradesPanel({ wallet, refreshKey }: { wallet: string; refreshKey: number }) {
    const fetcher = useCallback(() => api.pmTrades(wallet, 200), [wallet, refreshKey]);
    const { data, loading, error } = usePolling<PmTrade[]>(fetcher, 10_000, !!wallet);
    if (!wallet)
        return (
            <Card title="trades">
                <EmptyState loading={false} error={null} label="enter a wallet address" />
            </Card>
        );
    return (
        <Card title="trades" subtitle="data-api /trades (taker)">
            <TradesTable rows={data ?? []} loading={loading} error={error} />
        </Card>
    );
}

function PnlPanel({
    wallet,
    refreshKey,
    range,
    onRangeChange,
}: {
    wallet: string;
    refreshKey: number;
    range: PnlInterval;
    onRangeChange: (r: PnlInterval) => void;
}) {
    const fidelity = pnlFidelity(range);
    const fetcher = useCallback(
        () => api.pmPnl(wallet, range, fidelity),
        [wallet, range, fidelity, refreshKey],
    );
    const { data, loading, error } = usePolling<PmPnlPoint[]>(fetcher, 10_000, !!wallet);

    const seg = (active: boolean) =>
        `px-2.5 py-1 rounded-md text-[10px] font-medium border transition-colors ${
            active
                ? "text-white bg-bg-hover border-border-strong"
                : "text-text-muted border-transparent hover:text-text-primary hover:bg-bg-hover"
        }`;

    const controls = (
        <div className="flex gap-1">
            {(["1d", "1w", "1m", "all"] as const).map((w) => (
                <button key={w} onClick={() => onRangeChange(w)} className={seg(range === w)}>
                    {w}
                </button>
            ))}
        </div>
    );

    if (!wallet)
        return (
            <Card title="Cumulative P&L">
                <EmptyState loading={false} error={null} label="enter a wallet address" />
            </Card>
        );
    return (
        <Card title="Cumulative P&L" subtitle="user-pnl-api — profit / loss over time" action={controls}>
            <PnlChart points={data ?? []} loading={loading} error={error} />
        </Card>
    );
}

function ActivityPanel({ wallet, refreshKey }: { wallet: string; refreshKey: number }) {
    const fetcher = useCallback(
        () => api.pmActivity(wallet, undefined, 200),
        [wallet, refreshKey],
    );
    const { data, loading, error } = usePolling<PmActivity[]>(fetcher, 10_000, !!wallet);
    if (!wallet)
        return (
            <Card title="activity">
                <EmptyState loading={false} error={null} label="enter a wallet address" />
            </Card>
        );
    return (
        <Card title="activity" subtitle="data-api /activity">
            <ActivityTable rows={data ?? []} loading={loading} error={error} showType />
        </Card>
    );
}

// ── market-scoped panels ─────────────────────────────────────────────────────

function HoldersPanel({
    conditionId,
    setConditionId,
    onPickWallet,
    refreshKey,
}: {
    conditionId: string;
    setConditionId: (s: string) => void;
    onPickWallet: (w: string) => void;
    refreshKey: number;
}) {
    const fetcher = useCallback(() => api.pmHolders(conditionId), [conditionId, refreshKey]);
    const { data, loading, error } = usePolling<PmHolderGroup[]>(
        fetcher,
        10_000,
        conditionId.startsWith("0x"),
    );
    return (
        <Card
            title="holders"
            subtitle="data-api /holders"
            action={
                <input
                    type="text"
                    placeholder="market conditionId (0x…)"
                    value={conditionId}
                    onChange={(e) => setConditionId(e.target.value.trim())}
                    className="w-72 px-2 py-1 rounded border border-border-strong bg-bg-surface text-white placeholder:text-text-muted text-[10px] font-mono focus:outline-none focus:border-border-subtle"
                />
            }
        >
            {conditionId.startsWith("0x") ? (
                <HoldersTable
                    groups={data ?? []}
                    loading={loading}
                    error={error}
                    onPickWallet={onPickWallet}
                />
            ) : (
                <EmptyState loading={false} error={null} label="enter a market conditionId" />
            )}
        </Card>
    );
}

function MarketsPanel({
    slug,
    setSlug,
    refreshKey,
}: {
    slug: string;
    setSlug: (s: string) => void;
    refreshKey: number;
}) {
    const fetcher = useCallback(() => api.pmMarket(slug), [slug, refreshKey]);
    const { data, loading, error } = usePolling<PmMarketMetrics>(fetcher, 10_000, !!slug);
    return (
        <Card
            title="market metrics"
            subtitle="gamma /markets/slug — liquidity & volume"
            action={
                <input
                    type="text"
                    placeholder="market slug"
                    value={slug}
                    onChange={(e) => setSlug(e.target.value.trim())}
                    className="w-72 px-2 py-1 rounded border border-border-strong bg-bg-surface text-white placeholder:text-text-muted text-[10px] font-mono focus:outline-none focus:border-border-subtle"
                />
            }
        >
            <MarketMetrics data={slug ? data : null} loading={loading} error={error} />
        </Card>
    );
}

// ── leaderboard ──────────────────────────────────────────────────────────────

function LeaderboardPanel({
    metric,
    setMetric,
    window: win,
    setWindow,
    onPickWallet,
    refreshKey,
}: {
    metric: "volume" | "profit";
    setMetric: (m: "volume" | "profit") => void;
    window: "all" | "7d" | "30d";
    setWindow: (w: "all" | "7d" | "30d") => void;
    onPickWallet: (w: string) => void;
    refreshKey: number;
}) {
    const fetcher = useCallback(() => api.pmLeaderboard(metric, win, 25), [metric, win, refreshKey]);
    const { data, loading, error } = usePolling<PmLeaderboardEntry[]>(fetcher, 10_000);

    const seg = (active: boolean) =>
        `px-2.5 py-1 rounded-md text-[10px] font-medium border transition-colors ${
            active
                ? "text-white bg-bg-hover border-border-strong"
                : "text-text-muted border-transparent hover:text-text-primary hover:bg-bg-hover"
        }`;

    const controls = useMemo(
        () => (
            <div className="flex items-center gap-3">
                <div className="flex gap-1">
                    {(["volume", "profit"] as const).map((m) => (
                        <button key={m} onClick={() => setMetric(m)} className={seg(metric === m)}>
                            {m}
                        </button>
                    ))}
                </div>
                <div className="flex gap-1">
                    {(["all", "7d", "30d"] as const).map((w) => (
                        <button key={w} onClick={() => setWindow(w)} className={seg(win === w)}>
                            {w}
                        </button>
                    ))}
                </div>
            </div>
        ),
        [metric, win, setMetric, setWindow],
    );

    return (
        <Card title="leaderboard" subtitle="lb-api" action={controls}>
            <LeaderboardTable
                rows={data ?? []}
                loading={loading}
                error={error}
                metric={metric}
                onPickWallet={onPickWallet}
            />
        </Card>
    );
}
