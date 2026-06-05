import { useEffect, useMemo, useState } from "react";
import { Cpu, MemoryStick, HardDrive, Server, Activity, LineChart as LineChartIcon } from "lucide-react";
import {
    CartesianGrid,
    Line,
    LineChart,
    ResponsiveContainer,
    Tooltip,
    XAxis,
    YAxis,
} from "recharts";
import Card from "../components/Card";
import { usePolling } from "../lib/usePolling";
import { api, type MonitorStatus, type ServiceInfo } from "../lib/api";
import { C } from "../lib/colors";

// ── local formatters ────────────────────────────────────────────────────────

function fmtBytes(n: number): string {
    if (n <= 0) return "0 B";
    const units = ["B", "KB", "MB", "GB", "TB", "PB"];
    const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), units.length - 1);
    const v = n / 1024 ** i;
    return `${v.toFixed(v >= 100 || i === 0 ? 0 : 1)} ${units[i]}`;
}

function fmtDuration(secs: number): string {
    if (secs <= 0) return "—";
    const d = Math.floor(secs / 86400);
    const h = Math.floor((secs % 86400) / 3600);
    const m = Math.floor((secs % 3600) / 60);
    if (d > 0) return `${d}d ${h}h`;
    if (h > 0) return `${h}h ${m}m`;
    if (m > 0) return `${m}m`;
    return `${secs}s`;
}

// Local aliases onto the centralized accent tokens (see src/lib/colors.ts).
const GREEN = C.green;
const RED = C.red;
const BLUE = C.blue;

// Color ramp for usage bars: green < 70 < blue < 90 < red.
function usageColor(pct: number): string {
    if (pct >= 90) return RED;
    if (pct >= 70) return BLUE;
    return GREEN;
}

// ── small components ─────────────────────────────────────────────────────────

function UsageBar({ pct, label, detail }: { pct: number; label: string; detail?: string }) {
    const clamped = Math.max(0, Math.min(100, pct));
    return (
        <div className="space-y-1.5">
            <div className="flex items-baseline justify-between text-xs">
                <span style={{ color: C.textStrong }}>{label}</span>
                <span className="font-mono" style={{ color: C.textDim }}>
                    {detail ? `${detail} · ` : ""}
                    {clamped.toFixed(1)}%
                </span>
            </div>
            <div className="h-2 rounded-full overflow-hidden" style={{ background: C.borderCard }}>
                <div
                    className="h-full rounded-full transition-all duration-500"
                    style={{ width: `${clamped}%`, background: usageColor(clamped) }}
                />
            </div>
        </div>
    );
}

function ServiceRow({ s }: { s: ServiceInfo }) {
    const { dot, text } =
        s.error || s.active_state === "unknown"
            ? { dot: C.textGhost, text: C.textDim }
            : s.active_state === "active"
              ? { dot: GREEN, text: C.textStrong }
              : s.active_state === "failed"
                ? { dot: RED, text: RED }
                : s.active_state === "activating" || s.active_state === "deactivating"
                  ? { dot: BLUE, text: C.textStrong }
                  : { dot: C.textFaint, text: C.textDim };

    // Strip the common prefix/suffix for a denser label; keep full name in title.
    const short = s.unit.replace(/^velociraptor-/, "").replace(/\.service$/, "");

    return (
        <div
            className="flex items-center gap-3 px-4 py-2.5 border-b last:border-b-0"
            style={{ borderColor: C.borderCard }}
            title={s.unit}
        >
            <span
                className="w-2 h-2 rounded-full shrink-0"
                style={{ background: dot, boxShadow: `0 0 6px ${dot}` }}
            />
            <div className="flex-1 min-w-0">
                <p className="text-xs font-mono truncate" style={{ color: text }}>
                    {short}
                </p>
                {s.error && (
                    <p className="text-[10px] mt-0.5 truncate" style={{ color: C.textGhost }}>
                        {s.error}
                    </p>
                )}
            </div>
            <div className="text-right shrink-0">
                <p className="text-xs font-mono" style={{ color: text }}>
                    {s.active_state}
                    {s.sub_state ? ` / ${s.sub_state}` : ""}
                </p>
                <p className="text-[10px] font-mono" style={{ color: C.textGhost }}>
                    {s.main_pid > 0 ? `pid ${s.main_pid}` : "—"}
                    {s.active_secs > 0 ? ` · up ${fmtDuration(s.active_secs)}` : ""}
                </p>
            </div>
        </div>
    );
}

// ── history chart ────────────────────────────────────────────────────────────

interface ChartPoint {
    t: number; // unix seconds
    cpu: number;
    mem: number;
    disk: number; // busiest mount
}

/** Flatten a raw (newest-first) history into oldest-first chart points. */
function toChartPoints(history: MonitorStatus[]): ChartPoint[] {
    return history
        .map((h) => ({
            t: h.ts,
            cpu: Number(h.cpu.usage_pct.toFixed(1)),
            mem: Number(h.memory.used_pct.toFixed(1)),
            disk: Number(
                (h.disks.length ? Math.max(...h.disks.map((d) => d.used_pct)) : 0).toFixed(1),
            ),
        }))
        .sort((a, b) => a.t - b.t);
}

function fmtClock(t: number): string {
    return new Date(t * 1000).toLocaleTimeString("en-US", {
        hour12: false,
        hour: "2-digit",
        minute: "2-digit",
    });
}

const SERIES = [
    { key: "cpu", label: "CPU", color: GREEN },
    { key: "mem", label: "Memory", color: BLUE },
    { key: "disk", label: "Disk", color: RED },
] as const;

function HistoryChart({ points }: { points: ChartPoint[] }) {
    if (points.length < 2) {
        return (
            <div className="h-64 flex items-center justify-center">
                <p className="text-xs" style={{ color: C.textGhost }}>
                    collecting history… (a sample is recorded every 30s)
                </p>
            </div>
        );
    }
    return (
        <div className="h-64">
            <ResponsiveContainer width="100%" height="100%">
                <LineChart data={points} margin={{ top: 8, right: 12, bottom: 0, left: -16 }}>
                    <CartesianGrid stroke={C.borderCard} vertical={false} />
                    <XAxis
                        dataKey="t"
                        tickFormatter={fmtClock}
                        tick={{ fill: C.textSubtle, fontSize: 10 }}
                        stroke={C.borderStrong}
                        minTickGap={48}
                    />
                    <YAxis
                        domain={[0, 100]}
                        ticks={[0, 25, 50, 75, 100]}
                        tickFormatter={(v) => `${v}%`}
                        tick={{ fill: C.textSubtle, fontSize: 10 }}
                        stroke={C.borderStrong}
                        width={44}
                    />
                    <Tooltip
                        contentStyle={{
                            background: C.bgCard,
                            border: `1px solid ${C.borderStrong}`,
                            borderRadius: 6,
                            fontSize: 12,
                        }}
                        labelStyle={{ color: C.textDim }}
                        labelFormatter={(t) => fmtClock(Number(t))}
                        formatter={(v, name) => [`${Number(v)}%`, String(name)]}
                    />
                    {SERIES.map((s) => (
                        <Line
                            key={s.key}
                            type="monotone"
                            dataKey={s.key}
                            name={s.label}
                            stroke={s.color}
                            strokeWidth={1.5}
                            dot={false}
                            isAnimationActive={false}
                        />
                    ))}
                </LineChart>
            </ResponsiveContainer>
        </div>
    );
}

// ── page ─────────────────────────────────────────────────────────────────────

export default function Monitor() {
    // Host metrics are mildly expensive (CPU sampling + one systemctl per
    // unit), so poll on a slower cadence than the orderbook pages.
    const { data, error, loading } = usePolling<MonitorStatus>(() => api.monitor(), 5000);

    // History for the line graph — the sampler records every 30s, so polling
    // every 15s is plenty to pick up new points without redundant fetches.
    const { data: history } = usePolling<MonitorStatus[]>(() => api.monitorHistory(), 15000);
    const points = useMemo(() => toChartPoints(history ?? []), [history]);

    // Ticking clock (in unix seconds) so "updated Ns ago" stays live between
    // polls without reading the clock during render (React purity rule).
    const [nowSecs, setNowSecs] = useState(() => Math.floor(Date.now() / 1000));
    useEffect(() => {
        const id = setInterval(() => setNowSecs(Math.floor(Date.now() / 1000)), 1000);
        return () => clearInterval(id);
    }, []);

    if (loading && !data) {
        return (
            <div className="p-5 lg:px-8 w-full max-w-7xl mx-auto">
                <p className="text-xs" style={{ color: C.textDim }}>
                    loading system status…
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
                    monitor unavailable — {error}
                </div>
            </div>
        );
    }

    if (!data) return null;

    const { host, cpu, memory, disks, services } = data;
    const ageSecs = Math.max(0, nowSecs - data.ts);

    return (
        <div className="p-5 lg:px-8 w-full max-w-7xl mx-auto space-y-5">
            {/* Host header */}
            <div className="flex items-center justify-between flex-wrap gap-3">
                <div className="flex items-center gap-3">
                    <Server size={18} style={{ color: C.textDim }} />
                    <div>
                        <p className="text-sm font-medium" style={{ color: C.textBright }}>
                            {host.hostname}
                        </p>
                        <p className="text-[11px] font-mono" style={{ color: C.textSubtle }}>
                            {host.os} · {host.kernel} · {host.cpu_count} cores · up{" "}
                            {fmtDuration(host.uptime_secs)}
                        </p>
                    </div>
                </div>
                <p className="text-[11px] font-mono" style={{ color: ageSecs > 15 ? C.amber : C.textGhost }}>
                    updated {ageSecs}s ago
                </p>
            </div>

            {/* History — line graph of CPU / memory / busiest-disk over time */}
            <Card
                title="history"
                subtitle="cpu · memory · disk — sampled every 30s"
                action={
                    <div className="flex items-center gap-4">
                        {SERIES.map((s) => (
                            <span key={s.key} className="flex items-center gap-1.5">
                                <span
                                    className="w-2.5 h-0.5 rounded-full"
                                    style={{ background: s.color }}
                                />
                                <span className="text-[10px]" style={{ color: C.textDim }}>
                                    {s.label}
                                </span>
                            </span>
                        ))}
                        <LineChartIcon size={14} style={{ color: C.textSubtle }} />
                    </div>
                }
            >
                <HistoryChart points={points} />
            </Card>

            <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
                {/* CPU */}
                <Card
                    title="cpu"
                    subtitle={`load ${cpu.load_avg.map((l) => l.toFixed(2)).join(" / ")}`}
                    action={<Cpu size={14} style={{ color: C.textSubtle }} />}
                >
                    <div className="space-y-3">
                        <UsageBar pct={cpu.usage_pct} label="total" />
                        <div className="grid grid-cols-2 sm:grid-cols-4 xl:grid-cols-6 gap-x-4 gap-y-1.5 pt-1">
                            {cpu.per_core_pct.map((p, i) => (
                                <div key={i} className="flex items-center gap-2">
                                    <span
                                        className="text-[10px] font-mono w-6 shrink-0"
                                        style={{ color: C.textGhost }}
                                    >
                                        c{i}
                                    </span>
                                    <div
                                        className="h-1.5 flex-1 rounded-full overflow-hidden"
                                        style={{ background: C.borderCard }}
                                    >
                                        <div
                                            className="h-full rounded-full transition-all duration-500"
                                            style={{ width: `${p}%`, background: usageColor(p) }}
                                        />
                                    </div>
                                </div>
                            ))}
                        </div>
                    </div>
                </Card>

                {/* Memory */}
                <Card title="memory" action={<MemoryStick size={14} style={{ color: C.textSubtle }} />}>
                    <div className="space-y-3">
                        <UsageBar
                            pct={memory.used_pct}
                            label="RAM"
                            detail={`${fmtBytes(memory.used_bytes)} / ${fmtBytes(memory.total_bytes)}`}
                        />
                        {memory.swap_total_bytes > 0 && (
                            <UsageBar
                                pct={(memory.swap_used_bytes / memory.swap_total_bytes) * 100}
                                label="swap"
                                detail={`${fmtBytes(memory.swap_used_bytes)} / ${fmtBytes(memory.swap_total_bytes)}`}
                            />
                        )}
                        <p className="text-[11px] font-mono pt-1" style={{ color: C.textGhost }}>
                            {fmtBytes(memory.available_bytes)} available
                        </p>
                    </div>
                </Card>

                {/* Disks */}
                <Card title="disk" action={<HardDrive size={14} style={{ color: C.textSubtle }} />}>
                    <div className="space-y-3">
                        {disks.length === 0 ? (
                            <p className="text-xs" style={{ color: C.textGhost }}>
                                no disks reported
                            </p>
                        ) : (
                            disks.map((d) => (
                                <UsageBar
                                    key={d.mount_point}
                                    pct={d.used_pct}
                                    label={d.mount_point}
                                    detail={`${fmtBytes(d.used_bytes)} / ${fmtBytes(d.total_bytes)}`}
                                />
                            ))
                        )}
                    </div>
                </Card>

                {/* Services */}
                <Card
                    title="systemd services"
                    subtitle={`${services.filter((s) => s.active_state === "active").length}/${services.length} active`}
                    action={<Activity size={14} style={{ color: C.textSubtle }} />}
                    noPad
                >
                    {services.length === 0 ? (
                        <p className="text-xs p-4" style={{ color: C.textGhost }}>
                            no services reported
                        </p>
                    ) : (
                        services.map((s) => <ServiceRow key={s.unit} s={s} />)
                    )}
                </Card>
            </div>
        </div>
    );
}
