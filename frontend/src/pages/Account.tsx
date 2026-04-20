import { useState } from "react";
import Card from "../components/Card";
import Badge from "../components/Badge";
import { Search } from "lucide-react";

export default function AccountPage() {
    const [search, setSearch] = useState("");
    return (
        <div className="p-5 max-w-3xl mx-auto">
            {/* Stats */}
            <div className="grid grid-cols-3 gap-px mb-6 bg-border-strong overflow-hidden rounded-md shadow-sm">
                {[
                    { label: "open positions", value: "0" },
                    { label: "pending orders", value: "0" },
                    { label: "recent fills", value: "0" },
                ].map((s) => (
                    <div key={s.label} className="px-5 py-4 bg-bg-surface">
                        <p className="text-xs mb-1 font-medium text-text-muted uppercase tracking-wider">{s.label}</p>
                        <p className="text-xl font-mono font-bold text-white tracking-tight">{s.value}</p>
                    </div>
                ))}
            </div>

            <div className="flex items-center gap-4 mb-6">
                <div className="relative flex-1 max-w-md">
                    <Search size={14} className="absolute left-3 top-1/2 -translate-y-1/2 text-text-muted" />
                    <input
                        type="text"
                        placeholder="Search fills or orders..."
                        value={search}
                        onChange={(e) => setSearch(e.target.value)}
                        className="w-full pl-9 pr-4 py-2 rounded-md border border-border-strong bg-bg-surface text-white placeholder:text-text-muted text-xs font-mono focus:outline-none focus:border-border-subtle shadow-sm transition-all"
                    />
                </div>
            </div>

            <div className="grid grid-cols-2 gap-4 mb-4">
                <Card title="recent fills" subtitle="events:fills (Redis)">
                    <p className="text-xs py-10 text-center text-text-muted">
                        {search
                            ? `no fills found for "${search}"`
                            : "no fills — connect WS_STATUS_SOCKET for live events"}
                    </p>
                </Card>
                <Card title="order updates" subtitle="events:orders (Redis)">
                    <p className="text-xs py-10 text-center text-text-muted">
                        {search ? `no order updates found for "${search}"` : "no order updates"}
                    </p>
                </Card>
            </div>

            {/* UserEvent schema reference */}
            <Card title="UserEvent schema">
                <div className="grid grid-cols-2 gap-3">
                    {[
                        {
                            type: "fill",
                            fields: [
                                "exchange",
                                "client_oid",
                                "exchange_oid",
                                "symbol",
                                "side",
                                "px",
                                "qty",
                                "fee",
                                "ts_ns",
                            ],
                        },
                        {
                            type: "order_update",
                            fields: [
                                "exchange",
                                "client_oid",
                                "exchange_oid",
                                "symbol",
                                "side",
                                "px",
                                "qty",
                                "filled",
                                "status",
                                "ts_ns",
                            ],
                        },
                        { type: "balance", fields: ["exchange", "asset", "free", "locked", "ts_ns"] },
                        { type: "position", fields: ["exchange", "symbol", "size", "avg_px", "ts_ns"] },
                    ].map((s) => (
                        <div
                            key={s.type}
                            className="p-5 rounded-md border border-border-strong bg-bg-surface hover:border-border-subtle transition-colors shadow-sm"
                        >
                            <Badge label={s.type} variant="gray" />
                            <ul className="mt-4 font-mono text-[11px] text-text-muted opacity-80 space-y-2 list-none">
                                {s.fields.map((f) => (
                                    <li
                                        key={f}
                                        className="flex items-center before:content-[''] before:w-1 before:h-1 before:bg-border-strong before:rounded-full before:mr-3"
                                    >
                                        {f}
                                    </li>
                                ))}
                            </ul>
                        </div>
                    ))}
                </div>
            </Card>
        </div>
    );
}
