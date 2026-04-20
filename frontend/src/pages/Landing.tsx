import { Link } from "react-router-dom";
import { ArrowRight } from "lucide-react";

const exchanges = ["Binance", "OKX", "Hyperliquid", "Polymarket", "Kalshi"];

const features = [
    {
        title: "Multi-exchange orderbooks",
        desc: "Live bid/ask depth from crypto and prediction markets, unified stream via ZMQ PUB/SUB.",
    },
    {
        title: "Redis-backed state",
        desc: "Latest snapshots, BBA, trades, positions and balances persisted with capped history.",
    },
    { title: "Disk archival", desc: "Append-only MessagePack files, daily rotation, optional zstd compression." },
    { title: "ZMQ transport", desc: "PUB/SUB for market data, ROUTER/DEALER for subscriptions, REQ/REP for orders." },
    {
        title: "Rolling-window schedulers",
        desc: "Polymarket and Kalshi auto-rotate to new market windows every interval.",
    },
    { title: "HTTP backend", desc: "Axum REST API reads Redis and exposes JSON for any downstream client." },
];

export default function Landing() {
    return (
        <div className="flex flex-col items-center w-full max-w-4xl mx-auto pt-16 pb-12 gap-16 animate-in fade-in duration-500">
            {/* Hero */}
            <section className="text-center w-full flex flex-col items-center">
                <div className="inline-flex items-center gap-2 px-4 py-1.5 rounded-full border border-border-strong bg-bg-surface text-xs mb-8 font-mono shadow-sm">
                    <span className="w-2 h-2 rounded-full bg-accent-green animate-pulse" />
                    <span className="text-text-muted">{exchanges.join(" · ")}</span>
                </div>

                <h1 className="text-5xl font-bold leading-tight mb-6 tracking-tight text-white">
                    Market data infrastructure
                    <br />
                    <span className="text-text-muted font-normal">for prediction markets</span>
                </h1>

                <p className="text-base mb-10 max-w-2xl text-text-muted leading-relaxed">
                    Real-time orderbook streaming from crypto and prediction market exchanges. ZMQ transport, Redis
                    state, disk archival — all in one Rust workspace.
                </p>

                <div className="flex items-center">
                    <Link
                        to="/orderbook"
                        className="flex items-center gap-2.5 px-8 py-3 rounded-md border border-border-strong text-sm font-semibold text-white bg-bg-surface hover:bg-bg-hover hover:border-border-subtle transition-all shadow-sm group"
                    >
                        View Orderbook
                        <ArrowRight size={16} className="group-hover:translate-x-0.5 transition-transform" />
                    </Link>
                </div>
            </section>

            {/* Stats */}
            <section className="w-full">
                <div className="grid grid-cols-4 gap-4">
                    {[
                        { value: "5", label: "exchanges" },
                        { value: "<1ms", label: "latency target" },
                        { value: "IPC", label: "ZMQ transport" },
                        { value: "zstd", label: "compression" },
                    ].map((s) => (
                        <div
                            key={s.label}
                            className="p-6 text-center rounded-xl border border-border-strong bg-bg-surface hover:bg-bg-hover transition-colors shadow-sm"
                        >
                            <p className="text-2xl font-mono font-bold mb-1 text-white">{s.value}</p>
                            <p className="text-xs font-medium text-text-muted uppercase tracking-wider">{s.label}</p>
                        </div>
                    ))}
                </div>
            </section>

            {/* Features */}
            <section className="w-full">
                <div className="grid grid-cols-2 md:grid-cols-3 gap-4">
                    {features.map((f) => (
                        <div
                            key={f.title}
                            className="p-6 rounded-xl border border-border-strong bg-bg-surface hover:border-border-subtle transition-colors shadow-sm"
                        >
                            <h3 className="text-sm font-semibold mb-2 text-white">{f.title}</h3>
                            <p className="text-xs leading-relaxed text-text-muted">{f.desc}</p>
                        </div>
                    ))}
                </div>
            </section>

            <footer className="w-full border-t border-border-strong pt-8 mt-8 text-center text-xs font-mono text-text-muted">
                velociraptor · Rust · ZMQ · Redis · Axum
            </footer>
        </div>
    );
}
