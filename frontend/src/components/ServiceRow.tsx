// Shared systemd-service row + duration formatter, used by the Control page's
// services card (and previously the Monitor page). Kept here so both can render
// the same compact status row without duplication.

import type { ServiceInfo } from "../lib/api";
import { C } from "../lib/colors";

export function fmtDuration(secs: number): string {
    if (secs <= 0) return "—";
    const d = Math.floor(secs / 86400);
    const h = Math.floor((secs % 86400) / 3600);
    const m = Math.floor((secs % 3600) / 60);
    if (d > 0) return `${d}d ${h}h`;
    if (h > 0) return `${h}h ${m}m`;
    if (m > 0) return `${m}m`;
    return `${secs}s`;
}

export function ServiceRow({ s }: { s: ServiceInfo }) {
    const { dot, text } =
        s.error || s.active_state === "unknown"
            ? { dot: C.textGhost, text: C.textDim }
            : s.active_state === "active"
              ? { dot: C.green, text: C.textStrong }
              : s.active_state === "failed"
                ? { dot: C.red, text: C.red }
                : s.active_state === "activating" || s.active_state === "deactivating"
                  ? { dot: C.blue, text: C.textStrong }
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
