import { useEffect, useState } from "react";
import { Terminal } from "lucide-react";
import Card from "../components/Card";
import { api, type ControlStatus } from "../lib/api";

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

    async function sendControl(
        action: { type: "halt" | "resume" | "reload_risk" },
        label: string,
    ) {
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

    const btnClass =
        "px-4 py-2 rounded-md border border-border-strong bg-bg-surface text-text-muted hover:text-white hover:bg-bg-hover hover:border-border-subtle transition-all cursor-pointer shadow-sm font-mono text-xs flex items-center justify-center disabled:opacity-50 disabled:cursor-not-allowed";

    // ── Status banner ────────────────────────────────────────────────────────
    let banner: { text: string; cls: string };
    if (statusErr) {
        banner = { text: `status unavailable — ${statusErr}`, cls: "bg-bg-surface border-border-strong text-text-muted" };
    } else if (status?.kill_switch) {
        banner = { text: "HALTED — new orders blocked, all open orders cancelled", cls: "bg-accent-red/10 border-accent-red/30 text-accent-red" };
    } else if (status?.deadman_engaged) {
        banner = { text: "DEAD-MAN ENGAGED — backend heartbeat stale; executor self-blocked", cls: "bg-accent-blue/10 border-accent-blue/30 text-accent-blue" };
    } else if (status) {
        banner = { text: "LIVE — orders flowing normally", cls: "bg-accent-green/10 border-accent-green/30 text-accent-green" };
    } else {
        banner = { text: "loading control state…", cls: "bg-bg-surface border-border-strong text-text-muted" };
    }

    return (
        <div className="p-5 w-full max-w-4xl mx-auto space-y-6">
            <p className="text-xs text-text-muted leading-loose">
                HALT sets{" "}
                <code className="font-mono px-1.5 py-0.5 rounded-md border border-border-strong bg-bg-surface text-text-primary shadow-sm">
                    executor:kill_switch
                </code>{" "}
                +{" "}
                <code className="font-mono px-1.5 py-0.5 rounded-md border border-border-strong bg-bg-surface text-text-primary shadow-sm">
                    executor:cancel_all
                </code>{" "}
                via the backend. The executor cancels every open order on the wallet and blocks new
                ones until RESUME.
            </p>

            {/* Live status banner */}
            <div className={`px-4 py-3 rounded-md border font-mono text-xs font-medium ${banner.cls}`}>
                {banner.text}
            </div>

            <div className="grid grid-cols-1 gap-4 mb-4">
                <Card title="system control" subtitle="halt = block + cancel all" noPad>
                    <div className="p-4 flex gap-3 h-full items-center">
                        <button
                            disabled={busy}
                            onClick={() => sendControl({ type: "halt" }, "HALT — block + cancel all")}
                            className="flex-1 py-2.5 rounded-md border text-xs font-mono font-medium transition-all shadow-sm bg-accent-red/10 border-accent-red/20 text-accent-red hover:bg-accent-red/20 disabled:opacity-50 cursor-pointer"
                        >
                            HALT
                        </button>
                        <button
                            disabled={busy}
                            onClick={() => sendControl({ type: "resume" }, "resume")}
                            className={`flex-1 py-2.5 ${btnClass}`}
                        >
                            resume
                        </button>
                        <button
                            disabled={busy}
                            onClick={() => sendControl({ type: "reload_risk" }, "reload risk config")}
                            className={`flex-1 py-2.5 ${btnClass}`}
                        >
                            reload risk
                        </button>
                    </div>
                </Card>
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
        </div>
    );
}
