// Shared helpers + table components used by Fills and Orders sub-pages.
// Kept small and stylistically consistent across pages.

import { Search } from "lucide-react";
import Badge from "../../components/Badge";
import { fmtPrice, fmtQty } from "../../lib/format";
import type { UserEvent } from "../../lib/api";

export const POLL_INTERVAL_MS = 1000;
export const FETCH_LIMIT = 500;

// Display sizing knobs — bumped down from 11px to 10px on user request.
export const ROW_TEXT = "text-[10px]";
export const HEADER_TEXT = "text-[10px]";

// ── Helpers ──────────────────────────────────────────────────────────────────

export function fmtTimeNs(ns: number): string {
    if (!ns) return "—";
    return new Date(ns / 1e6).toLocaleTimeString("en-US", { hour12: false });
}

export function statusVariant(status: string): "gray" | "green" | "red" | "yellow" {
    switch (status) {
        case "filled":
            return "green";
        case "canceled":
        case "expired":
            return "yellow";
        case "rejected":
            return "red";
        default:
            return "gray";
    }
}

export function eventMatchesSearch(ev: UserEvent, query: string): boolean {
    if (!query) return true;
    const needle = query.toLowerCase();
    if ("symbol" in ev && ev.symbol?.toLowerCase().includes(needle)) return true;
    if ("exchange_oid" in ev && ev.exchange_oid?.toLowerCase().includes(needle)) return true;
    if ("client_oid" in ev && ev.client_oid?.toLowerCase().includes(needle)) return true;
    return false;
}

// ── UI atoms ─────────────────────────────────────────────────────────────────

export function SearchBox({
    value,
    onChange,
    placeholder = "Search by symbol or order id...",
}: {
    value: string;
    onChange: (s: string) => void;
    placeholder?: string;
}) {
    return (
        <div className="flex items-center gap-4 mb-4">
            <div className="relative flex-1 max-w-md">
                <Search size={12} className="absolute left-3 top-1/2 -translate-y-1/2 text-text-muted" />
                <input
                    type="text"
                    placeholder={placeholder}
                    value={value}
                    onChange={(e) => onChange(e.target.value)}
                    className="w-full pl-8 pr-3 py-1.5 rounded-md border border-border-strong bg-bg-surface text-white placeholder:text-text-muted text-[11px] font-mono focus:outline-none focus:border-border-subtle shadow-sm transition-all"
                />
            </div>
            <p className="text-[9px] font-mono text-text-muted">
                polling every {POLL_INTERVAL_MS / 1000}s · limit {FETCH_LIMIT}
            </p>
        </div>
    );
}

export function EmptyState({ loading, error, label }: { loading: boolean; error: string | null; label: string }) {
    if (loading) return <p className={`${ROW_TEXT} py-10 text-center text-text-muted`}>loading…</p>;
    if (error)
        return (
            <p className={`${ROW_TEXT} py-10 text-center text-text-muted`}>
                <span className="text-red-400">error:</span> {error}
            </p>
        );
    return <p className={`${ROW_TEXT} py-10 text-center text-text-muted`}>{label}</p>;
}

// ── Tables ───────────────────────────────────────────────────────────────────

export function FillsTable({
    events,
    loading,
    error,
}: {
    events: UserEvent[];
    loading: boolean;
    error: string | null;
}) {
    if (events.length === 0) return <EmptyState loading={loading} error={error} label="no fills" />;
    return (
        <div className="overflow-x-auto">
            <table className={`w-full table-auto ${ROW_TEXT} font-mono`}>
                <thead>
                    <tr className={`text-text-muted text-left ${HEADER_TEXT}`}>
                        <th className="py-1.5 pr-3 font-medium whitespace-nowrap">time</th>
                        <th className="py-1.5 pr-3 font-medium whitespace-nowrap">exchange</th>
                        <th className="py-1.5 pr-3 font-medium whitespace-nowrap">side</th>
                        <th className="py-1.5 pr-3 font-medium text-right whitespace-nowrap">px</th>
                        <th className="py-1.5 pr-3 font-medium text-right whitespace-nowrap">qty</th>
                        <th className="py-1.5 pr-3 font-medium text-right whitespace-nowrap">fee</th>
                        <th className="py-1.5 pr-3 font-medium">symbol</th>
                        <th className="py-1.5 pr-3 font-medium">order id</th>
                        <th className="py-1.5 font-medium">client id</th>
                    </tr>
                </thead>
                <tbody>
                    {events.map((ev, i) =>
                        ev.type === "fill" ? (
                            <tr key={`${ev.exchange_oid}-${ev.ts_ns}-${i}`} className="border-t border-border-strong">
                                <td className="py-1 pr-3 text-text-muted whitespace-nowrap">{fmtTimeNs(ev.ts_ns)}</td>
                                <td className="py-1 pr-3 text-text-muted whitespace-nowrap">{ev.exchange}</td>
                                <td className="py-1 pr-3 whitespace-nowrap">
                                    <Badge label={ev.side} variant={ev.side === "buy" ? "green" : "red"} />
                                </td>
                                <td className="py-1 pr-3 text-right text-white whitespace-nowrap">{fmtPrice(ev.px)}</td>
                                <td className="py-1 pr-3 text-right text-white whitespace-nowrap">{fmtQty(ev.qty)}</td>
                                <td className="py-1 pr-3 text-right text-text-muted whitespace-nowrap">
                                    {fmtPrice(ev.fee, 6)}
                                </td>
                                <td className="py-1 pr-3 text-text-muted truncate max-w-[28ch]" title={ev.symbol}>
                                    {ev.symbol}
                                </td>
                                <td className="py-1 pr-3 text-text-muted truncate max-w-[28ch]" title={ev.exchange_oid}>
                                    {ev.exchange_oid}
                                </td>
                                <td
                                    className="py-1 text-text-muted truncate max-w-[24ch]"
                                    title={ev.client_oid ?? ""}
                                >
                                    {ev.client_oid ?? ""}
                                </td>
                            </tr>
                        ) : null,
                    )}
                </tbody>
            </table>
        </div>
    );
}

export function OrdersTable({
    events,
    loading,
    error,
}: {
    events: UserEvent[];
    loading: boolean;
    error: string | null;
}) {
    if (events.length === 0) return <EmptyState loading={loading} error={error} label="no order updates" />;
    return (
        <div className="overflow-x-auto">
            <table className={`w-full table-auto ${ROW_TEXT} font-mono`}>
                <thead>
                    <tr className={`text-text-muted text-left ${HEADER_TEXT}`}>
                        <th className="py-1.5 pr-3 font-medium whitespace-nowrap">time</th>
                        <th className="py-1.5 pr-3 font-medium whitespace-nowrap">exchange</th>
                        <th className="py-1.5 pr-3 font-medium whitespace-nowrap">status</th>
                        <th className="py-1.5 pr-3 font-medium whitespace-nowrap">side</th>
                        <th className="py-1.5 pr-3 font-medium text-right whitespace-nowrap">px</th>
                        <th className="py-1.5 pr-3 font-medium text-right whitespace-nowrap">qty</th>
                        <th className="py-1.5 pr-3 font-medium text-right whitespace-nowrap">filled</th>
                        <th className="py-1.5 pr-3 font-medium">symbol</th>
                        <th className="py-1.5 pr-3 font-medium">order id</th>
                        <th className="py-1.5 font-medium">client id</th>
                    </tr>
                </thead>
                <tbody>
                    {events.map((ev, i) =>
                        ev.type === "order_update" ? (
                            <tr key={`${ev.exchange_oid}-${ev.ts_ns}-${i}`} className="border-t border-border-strong">
                                <td className="py-1 pr-3 text-text-muted whitespace-nowrap">{fmtTimeNs(ev.ts_ns)}</td>
                                <td className="py-1 pr-3 text-text-muted whitespace-nowrap">{ev.exchange}</td>
                                <td className="py-1 pr-3 whitespace-nowrap">
                                    <Badge label={ev.status} variant={statusVariant(ev.status)} />
                                </td>
                                <td className="py-1 pr-3 whitespace-nowrap">
                                    <Badge label={ev.side} variant={ev.side === "buy" ? "green" : "red"} />
                                </td>
                                <td className="py-1 pr-3 text-right text-white whitespace-nowrap">{fmtPrice(ev.px)}</td>
                                <td className="py-1 pr-3 text-right text-white whitespace-nowrap">{fmtQty(ev.qty)}</td>
                                <td className="py-1 pr-3 text-right text-text-muted whitespace-nowrap">
                                    {fmtQty(ev.filled)}
                                </td>
                                <td className="py-1 pr-3 text-text-muted truncate max-w-[28ch]" title={ev.symbol}>
                                    {ev.symbol}
                                </td>
                                <td className="py-1 pr-3 text-text-muted truncate max-w-[28ch]" title={ev.exchange_oid}>
                                    {ev.exchange_oid}
                                </td>
                                <td
                                    className="py-1 text-text-muted truncate max-w-[24ch]"
                                    title={ev.client_oid ?? ""}
                                >
                                    {ev.client_oid ?? ""}
                                </td>
                            </tr>
                        ) : null,
                    )}
                </tbody>
            </table>
        </div>
    );
}

// ── Tab strip shared between sub-pages ───────────────────────────────────────

import { NavLink } from "react-router-dom";

export function AccountTabs() {
    const tabs = [
        { to: "/account/fills", label: "Fills" },
        { to: "/account/orders", label: "Order Updates" },
    ];
    return (
        <div className="flex items-center gap-1 mb-4">
            {tabs.map(({ to, label }) => (
                <NavLink
                    key={to}
                    to={to}
                    className={({ isActive }) =>
                        `px-3 py-1 rounded-md text-[11px] font-medium border transition-colors ${
                            isActive
                                ? "text-white bg-bg-hover border-border-strong"
                                : "text-text-muted border-transparent hover:text-text-primary hover:bg-bg-hover hover:border-border-subtle"
                        }`
                    }
                >
                    {label}
                </NavLink>
            ))}
        </div>
    );
}
