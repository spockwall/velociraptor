import { useState } from "react";
import { Terminal } from "lucide-react";
import Card from "../components/Card";

interface LogEntry {
    ts: string;
    level: "ok" | "err" | "info";
    msg: string;
}

async function sendControl(msg: object): Promise<void> {
    const res = await fetch("/api/control", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(msg),
    });
    if (!res.ok) {
        const body = await res.json().catch(() => ({}));
        throw new Error((body as { error?: string }).error ?? `HTTP ${res.status}`);
    }
}

export default function ControlPage() {
    const [log, setLog] = useState<LogEntry[]>([]);
    const [params, setParams] = useState("");
    const [busy, setBusy] = useState(false);

    function pushLog(level: LogEntry["level"], msg: string) {
        const ts = new Date().toLocaleTimeString("en-US", { hour12: false });
        setLog((l) => [{ ts, level, msg }, ...l].slice(0, 60));
    }

    async function send(msg: object, label: string) {
        setBusy(true);
        pushLog("info", `→ ${label}`);
        try {
            await sendControl(msg);
            pushLog("ok", `✓ ${label}`);
        } catch (e) {
            pushLog("err", `✗ ${e instanceof Error ? e.message : String(e)}`);
        } finally {
            setBusy(false);
        }
    }

    const btnClass =
        "px-4 py-2 rounded-md border border-border-strong bg-bg-surface text-text-muted hover:text-white hover:bg-bg-hover hover:border-border-subtle transition-all cursor-pointer shadow-sm font-mono text-xs flex items-center justify-center disabled:opacity-50 disabled:cursor-not-allowed";

    return (
        <div className="p-5 w-full max-w-4xl mx-auto space-y-6">
            {/* Warning */}
            <p className="text-xs text-text-muted flex flex-wrap items-center gap-1.5 leading-loose">
                Broadcasts{" "}
                <code className="font-mono px-1.5 py-0.5 rounded-md border border-border-strong bg-bg-surface text-text-primary shadow-sm">
                    ControlMessage
                </code>{" "}
                via{" "}
                <code className="font-mono px-1.5 py-0.5 rounded-md border border-border-strong bg-bg-surface text-text-primary shadow-sm">
                    CONTROL_SOCKET · ipc:///tmp/trading/control.sock
                </code>
            </p>

            <div className="grid grid-cols-1 gap-4 mb-4">
                <Card title="system control" subtitle="global state overrides" noPad>
                    <div className="p-4 flex gap-3 h-full items-center">
                        <button
                            disabled={busy}
                            onClick={() => send({ type: "pause" }, "pause")}
                            className={`flex-1 py-2.5 ${btnClass}`}
                        >
                            pause
                        </button>
                        <button
                            disabled={busy}
                            onClick={() => send({ type: "resume" }, "resume")}
                            className={`flex-1 py-2.5 ${btnClass}`}
                        >
                            resume
                        </button>
                        <button
                            disabled={busy}
                            onClick={() => send({ type: "shutdown" }, "shutdown")}
                            className="flex-1 py-2.5 rounded-md border text-xs font-mono font-medium transition-all shadow-sm bg-accent-red/10 border-accent-red/20 text-accent-red hover:bg-accent-red/20 disabled:opacity-50 cursor-pointer"
                        >
                            shutdown
                        </button>
                    </div>
                </Card>
            </div>

            {/* Strategy params */}
            <Card title="strategy params" subtitle="hot-reload engine parameters">
                <div className="flex gap-3">
                    <textarea
                        rows={3}
                        value={params}
                        onChange={(e) => setParams(e.target.value)}
                        placeholder='{"max_pos": 1000, "threshold": 0.01}'
                        className="flex-1 px-4 py-3 rounded-md border border-border-strong bg-bg-surface text-white text-xs font-mono resize-none focus:outline-none focus:border-border-subtle shadow-inner inset-0"
                    />
                    <button
                        disabled={busy}
                        onClick={() => {
                            try {
                                const parsed = JSON.parse(params);
                                send({ type: "strategy_params", ...parsed }, "strategy_params");
                            } catch {
                                pushLog("err", "invalid JSON");
                            }
                        }}
                        className={btnClass}
                    >
                        send
                    </button>
                </div>
            </Card>

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
