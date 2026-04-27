import { useState } from "react";
import { ChevronDown } from "lucide-react";

export const PRESETS = [
    { exchange: "binance", symbol: "btcusdt", label: "Binance · BTCUSDT" },
    { exchange: "binance", symbol: "ethusdt", label: "Binance · ETHUSDT" },
    { exchange: "binance_spot", symbol: "btcusdt", label: "Binance Spot · BTCUSDT" },
    { exchange: "binance_spot", symbol: "ethusdt", label: "Binance Spot · ETHUSDT" },
    { exchange: "okx", symbol: "BTC-USDT", label: "OKX · BTC-USDT" },
    { exchange: "okx", symbol: "ETH-USDT-SWAP", label: "OKX · ETH-USDT-SWAP" },
    { exchange: "hyperliquid", symbol: "BTC", label: "Hyperliquid · BTC" },
    { exchange: "hyperliquid", symbol: "ETH", label: "Hyperliquid · ETH" },
];

interface Props {
    exchange: string;
    symbol: string;
    onChange: (exchange: string, symbol: string) => void;
}

export default function ExchangeSymbolPicker({ exchange, symbol, onChange }: Props) {
    const [open, setOpen] = useState(false);
    const current = PRESETS.find((p) => p.exchange === exchange && p.symbol === symbol);
    const label = current?.label ?? `${exchange} · ${symbol}`;

    return (
        <div className="relative">
            <button
                onClick={() => setOpen((v) => !v)}
                className="flex items-center gap-2 px-3 py-1.5 rounded-md border border-border-strong bg-bg-surface hover:bg-bg-hover hover:border-border-subtle text-text-muted hover:text-text-primary text-xs font-mono transition-all shadow-sm"
            >
                {label}
                <ChevronDown size={14} className="opacity-70" />
            </button>

            {open && (
                <>
                    <div className="fixed inset-0 z-10" onClick={() => setOpen(false)} />
                    <div className="absolute left-0 mt-2 z-20 rounded-md border border-border-strong bg-bg-surface py-1 min-w-full shadow-xl overflow-hidden animate-in fade-in slide-in-from-top-1 duration-200">
                        {PRESETS.map((p) => {
                            const isActive = p.exchange === exchange && p.symbol === symbol;
                            return (
                                <button
                                    key={`${p.exchange}/${p.symbol}`}
                                    onClick={() => {
                                        onChange(p.exchange, p.symbol);
                                        setOpen(false);
                                    }}
                                    className={`w-full text-left px-4 py-2 text-xs font-mono whitespace-nowrap transition-colors ${
                                        isActive
                                            ? "text-white bg-bg-hover border-l-2 border-accent-green"
                                            : "text-text-muted hover:text-text-primary hover:bg-bg-hover border-l-2 border-transparent"
                                    }`}
                                >
                                    {p.label}
                                </button>
                            );
                        })}
                    </div>
                </>
            )}
        </div>
    );
}
