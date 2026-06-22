import { useEffect, useState } from "react";
import { Terminal, Activity } from "lucide-react";
import Card from "../components/Card";
import { ServiceRow } from "../components/ServiceRow";
import { C } from "../lib/colors";
import { api, type ControlStatus, type ServiceInfo } from "../lib/api";

interface LogEntry {
    ts: string;
    level: "ok" | "err" | "info";
    msg: string;
}

export default function ControlPage() {
    const [log, setLog] = useState<LogEntry[]>([]);
    const [busy, setBusy] = useState(false);
    const [status, setStatus] = useState<ControlStatus | null>(null);
    const [statusErr, setStatusErr] = useState<string | null>(null);
    const [services, setServices] = useState<ServiceInfo[]>([]);

    function pushLog(level: LogEntry["level"], msg: string) {
        const ts = new Date().toLocaleTimeString("en-US", { hour12: false });
        setLog((l) => [{ ts, level, msg }, ...l].slice(0, 60));
    }

    // Poll executor control state every 2s (poll-based, matching the rest of
    // the app — no SSE).
    useEffect(() => {
        let alive = true;
        async function tick() {
            try {
                const s = await api.getControl();
                if (!alive) return;
                setStatus(s);
                setStatusErr(null);
            } catch (e) {
                if (!alive) return;
                setStatusErr(e instanceof Error ? e.message : String(e));
            }
        }
        tick();
        const id = setInterval(tick, 2000);
        return () => {
            alive = false;
            clearInterval(id);
        };
    }, []);

    // Poll systemd service status (from /api/monitor) every 5s — the same
    // source the Monitor page used; collecting it is mildly expensive.
    useEffect(() => {
        let alive = true;
        async function tick() {
            try {
                const m = await api.monitor();
                if (alive) setServices(m.services);
            } catch {
                /* leave previous list on transient error */
            }
        }
        tick();
        const id = setInterval(tick, 5000);
        return () => {
            alive = false;
            clearInterval(id);
        };
    }, []);

    async function sendControl(action: { type: "halt" | "resume" | "reload_risk" }, label: string) {
        setBusy(true);
        pushLog("info", `→ ${label}`);
        try {
            const s = await api.postControl(action);
            setStatus(s);
            pushLog("ok", `✓ ${label}`);
        } catch (e) {
            pushLog("err", `✗ ${e instanceof Error ? e.message : String(e)}`);
        } finally {
            setBusy(false);
        }
    }

    // Solid, bright action buttons. Base layout is shared; each variant only
    // swaps the fill / hover-fill (the bright accent tokens live in index.css).
    const btnBase =
        "flex-1 min-w-[7rem] py-3 rounded-md text-sm font-mono font-semibold text-white shadow-sm transition-all cursor-pointer flex items-center justify-center disabled:opacity-50 disabled:cursor-not-allowed";
    const haltBtn = `${btnBase} bg-accent-red-bright hover:bg-accent-red-bright-hover`;
    const resumeBtn = `${btnBase} bg-accent-green-bright hover:bg-accent-green-bright-hover`;
    // reloadBtn — kept commented alongside the disabled "reload risk" button below.
    // const reloadBtn = `${btnBase} bg-accent-blue-bright hover:bg-accent-blue-bright-hover`;

    // ── Status badge ─────────────────────────────────────────────────────────
    // Compact state pill — just the state word; full detail is in the title
    // tooltip + the explanation text.
    let badge: { text: string; cls: string; title: string };
    if (statusErr) {
        badge = {
            text: "UNKNOWN",
            cls: "bg-bg-surface border-border-strong text-text-muted",
            title: `status unavailable — ${statusErr}`,
        };
    } else if (status?.kill_switch) {
        badge = {
            text: "HALTED",
            cls: "bg-accent-red/15 border-accent-red/40 text-accent-red",
            title: "new orders blocked, all open orders cancelled",
        };
    } else if (status?.deadman_engaged) {
        badge = {
            text: "DEAD-MAN",
            cls: "bg-accent-blue/15 border-accent-blue/40 text-accent-blue",
            title: "backend heartbeat stale; executor self-blocked",
        };
    } else if (status) {
        badge = {
            text: "LIVE",
            cls: "bg-accent-green/15 border-accent-green/40 text-accent-green",
            title: "orders flowing normally",
        };
    } else {
        badge = { text: "…", cls: "bg-bg-surface border-border-strong text-text-muted", title: "loading control state" };
    }

    return (
        <div className="p-5 lg:px-8 w-full max-w-7xl mx-auto space-y-5">
            {/* Top row — compact status badge (left), explanation, and actions
                side by side, wrapping to a stack on narrow windows. */}
            <div className="flex flex-col lg:flex-row lg:items-center gap-4">
                {/* Live status badge — compact, left-aligned, shrink-to-fit */}
                <div
                    title={badge.title}
                    className={`shrink-0 self-start lg:self-center px-3 py-1.5 rounded-md border font-mono text-xs font-semibold tracking-wide ${badge.cls}`}
                >
                    {badge.text}
                </div>

                {/* Explanation */}
                <p className="flex-1 text-xs text-text-muted leading-loose">
                    HALT sets{" "}
                    <code className="font-mono px-1.5 py-0.5 rounded-md border border-border-strong bg-bg-surface text-text-primary shadow-sm">
                        executor:kill_switch+cancel_all
                    </code>{" "}
                    + via the backend. The executor cancels every open order on the wallet and blocks new ones until
                    RESUME.
                </p>

                {/* Action buttons */}
                <div className="shrink-0 flex flex-wrap gap-3 items-stretch">
                    <button
                        disabled={busy}
                        onClick={() => sendControl({ type: "halt" }, "HALT — block + cancel all")}
                        className={haltBtn}
                    >
                        HALT
                    </button>
                    <button
                        disabled={busy}
                        onClick={() => sendControl({ type: "resume" }, "resume")}
                        className={resumeBtn}
                    >
                        resume
                    </button>
                    {/*<button
                        disabled={busy}
                        onClick={() => sendControl({ type: "reload_risk" }, "reload risk config")}
                        className={reloadBtn}
                    >
                        reload risk
                    </button>*/}
                </div>
            </div>

            {/* Log */}
            <Card title="log" noPad>
                <div className="h-48 overflow-y-auto p-4 font-mono bg-bg-base/50">
                    {log.length === 0 ? (
                        <div className="flex flex-col items-center justify-center h-full gap-3 text-text-muted opacity-60">
                            <Terminal size={24} />
                            <span className="text-xs uppercase tracking-widest font-medium">no commands sent</span>
                        </div>
                    ) : (
                        log.map((e, i) => (
                            <div key={i} className="flex gap-4 text-xs py-1">
                                <span className="text-text-muted opacity-80 shrink-0">{e.ts}</span>
                                <span
                                    className={`
                ${e.level === "ok" ? "text-accent-green" : ""}
                ${e.level === "err" ? "text-accent-red font-medium" : ""}
                ${e.level === "info" ? "text-text-primary" : ""}
              `}
                                >
                                    {e.msg}
                                </span>
                            </div>
                        ))
                    )}
                </div>
            </Card>

            {/* Systemd services — moved here from the Monitor page */}
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
    );
}
