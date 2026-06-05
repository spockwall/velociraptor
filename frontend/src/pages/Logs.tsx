import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, ScrollText } from "lucide-react";
import Card from "../components/Card";
import { usePolling } from "../lib/usePolling";
import { api, type LogEntry } from "../lib/api";
import { C } from "../lib/colors";

// Level → color. ERROR is red, WARN a muted gold; other levels stay grey.
function levelColor(level: string | null): string {
    if (level === "ERROR") return C.red;
    if (level === "WARN") return C.gold;
    return C.textDim;
}

function fmtTime(ts: string | null): string {
    if (!ts) return "—";
    // RFC3339 → HH:MM:SS (drop date + sub-seconds for a compact column).
    const m = ts.match(/T(\d{2}:\d{2}:\d{2})/);
    return m ? m[1] : ts;
}

// One log row — fixed-width meta columns, message wraps.
function LogRow({ e }: { e: LogEntry }) {
    return (
        <div
            className="grid items-baseline gap-3 px-3 py-1.5 border-b font-mono text-[11px] leading-relaxed"
            style={{
                gridTemplateColumns: "64px 52px 110px 1fr",
                borderColor: C.borderFaint,
            }}
        >
            <span style={{ color: C.textSubtle }}>{fmtTime(e.ts)}</span>
            <span style={{ color: levelColor(e.level), fontWeight: 600 }}>
                {e.level ?? "·"}
            </span>
            <span className="truncate" style={{ color: C.blue }} title={e.service}>
                {e.service}
            </span>
            <span style={{ color: C.textStrong, wordBreak: "break-word" }}>
                {e.target ? <span style={{ color: C.textFaint }}>{e.target}: </span> : null}
                {messageOf(e)}
            </span>
        </div>
    );
}

// Strip the leading "<ts> LEVEL target:" prefix from `raw` when those fields
// parsed, so the message column isn't redundant. Falls back to full `raw`.
function messageOf(e: LogEntry): string {
    let s = e.raw;
    if (e.ts && s.startsWith(e.ts)) s = s.slice(e.ts.length).trimStart();
    if (e.level && s.startsWith(e.level)) s = s.slice(e.level.length).trimStart();
    if (e.target && s.startsWith(`${e.target}:`)) s = s.slice(e.target.length + 1).trimStart();
    return s;
}

export default function Logs() {
    // Error logs are cheap to read (one capped Redis LRANGE) but low-churn —
    // the tailer only appends on new WARN/ERROR output — so a 5s poll is plenty.
    const { data, error, loading, lastUpdated } = usePolling<LogEntry[]>(
        () => api.errorLogs(),
        5000,
    );

    // Service filter — null = all.
    const [service, setService] = useState<string | null>(null);

    // Ticking clock so "updated Ns ago" stays live between polls (React purity:
    // never read Date.now() during render). `lastUpdated` comes from the poller.
    const [nowSecs, setNowSecs] = useState(() => Math.floor(Date.now() / 1000));
    useEffect(() => {
        const id = setInterval(() => setNowSecs(Math.floor(Date.now() / 1000)), 1000);
        return () => clearInterval(id);
    }, []);

    const services = useMemo(() => {
        const set = new Set<string>();
        (data ?? []).forEach((e) => set.add(e.service));
        return Array.from(set).sort();
    }, [data]);

    const rows = useMemo(
        () => (data ?? []).filter((e) => service === null || e.service === service),
        [data, service],
    );

    if (loading && !data) {
        return (
            <div className="p-5 lg:px-8 w-full max-w-7xl mx-auto">
                <p className="text-xs" style={{ color: C.textDim }}>
                    loading error logs…
                </p>
            </div>
        );
    }

    if (error && !data) {
        return (
            <div className="p-5 lg:px-8 w-full max-w-7xl mx-auto">
                <div
                    className="px-4 py-3 rounded-md border font-mono text-xs"
                    style={{ background: C.errorBg, borderColor: C.errorBorder, color: C.errorText }}
                >
                    logs unavailable — {error}
                </div>
            </div>
        );
    }

    const ageSecs = lastUpdated === null ? 0 : Math.max(0, nowSecs - lastUpdated);

    return (
        <div className="p-5 lg:px-8 w-full max-w-7xl mx-auto space-y-4">
            {/* Header */}
            <div className="flex items-center justify-between flex-wrap gap-3">
                <div className="flex items-center gap-3">
                    <ScrollText size={18} style={{ color: C.textDim }} />
                    <div>
                        <p className="text-sm font-medium" style={{ color: C.textBright }}>
                            Error logs
                        </p>
                        <p className="text-[11px] font-mono" style={{ color: C.textSubtle }}>
                            WARN+ across all services · newest first
                        </p>
                    </div>
                </div>
                <p
                    className="text-[11px] font-mono"
                    style={{ color: ageSecs > 10 ? C.gold : C.textGhost }}
                >
                    updated {ageSecs}s ago
                </p>
            </div>

            {/* Service filter chips */}
            <div className="flex items-center gap-2 flex-wrap">
                <FilterChip
                    label="all"
                    active={service === null}
                    onClick={() => setService(null)}
                />
                {services.map((s) => (
                    <FilterChip
                        key={s}
                        label={s}
                        active={service === s}
                        onClick={() => setService(s)}
                    />
                ))}
            </div>

            <Card
                title="recent errors"
                subtitle={`${rows.length} entries`}
                action={<AlertTriangle size={14} style={{ color: C.textSubtle }} />}
                noPad
            >
                {rows.length === 0 ? (
                    <p className="px-4 py-6 text-center text-xs" style={{ color: C.textGhost }}>
                        no errors logged — all clear
                    </p>
                ) : (
                    <div className="max-h-[70vh] overflow-y-auto">
                        {/* Column header */}
                        <div
                            className="grid gap-3 px-3 py-1.5 border-b text-[10px] uppercase tracking-wide sticky top-0"
                            style={{
                                gridTemplateColumns: "64px 52px 110px 1fr",
                                borderColor: C.borderCard,
                                background: C.bgCard,
                                color: C.textGhost,
                            }}
                        >
                            <span>time</span>
                            <span>level</span>
                            <span>service</span>
                            <span>message</span>
                        </div>
                        {rows.map((e, i) => (
                            <LogRow key={`${e.service}-${e.ts ?? ""}-${i}`} e={e} />
                        ))}
                    </div>
                )}
            </Card>
        </div>
    );
}

function FilterChip({
    label,
    active,
    onClick,
}: {
    label: string;
    active: boolean;
    onClick: () => void;
}) {
    return (
        <button
            type="button"
            onClick={onClick}
            className="px-2.5 py-1 rounded text-[11px] font-mono border transition-colors"
            style={
                active
                    ? { background: C.bgChip, borderColor: C.borderChip, color: C.textBright }
                    : { background: "transparent", borderColor: C.borderStrong, color: C.textDim }
            }
        >
            {label}
        </button>
    );
}
