use clap::Parser;
use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct BinanceConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub symbols: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct OkxConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub symbols: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct HyperliquidConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub coins: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct KalshiConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Kalshi market tickers, e.g. `["FED-23DEC-T3.00", "PRES-2028"]`.
    #[serde(default)]
    pub tickers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PolymarketMarket {
    pub enabled: bool,
    /// Base slug, e.g. `"btc-updown-15m"` or a full static slug.
    pub slug: String,
    /// Window size in seconds. 0 = static market (slug used as-is).
    pub interval_secs: u64,
}

impl Default for PolymarketMarket {
    fn default() -> Self {
        Self {
            enabled: true,
            slug: String::new(),
            interval_secs: 0,
        }
    }
}

/// Settings shared by the `polymarket_orderbook` visualiser and `polymarket_recorder` binary.
/// Maps to the `[server]` section in `configs/polymarket.toml`.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct PolymarketConfig {
    pub depth: usize,
    pub render_interval: u64,
    /// Root directory for recorded files (recorder only).
    pub base_path: String,
    /// BufWriter flush interval in milliseconds (recorder only).
    pub flush_interval: u64,
    /// zstd compression level after window close. 0 = disabled (recorder only).
    pub zstd_level: u8,
}

impl Default for PolymarketConfig {
    fn default() -> Self {
        Self {
            depth: 10,
            render_interval: 300,
            base_path: "./data".into(),
            flush_interval: 1000,
            zstd_level: 0,
        }
    }
}

/// CLI arguments shared by `polymarket_orderbook` and `polymarket_recorder`.
#[derive(Parser, Debug)]
pub struct PolymarketArgs {
    /// Path to TOML config file
    #[clap(long)]
    pub config: Option<String>,

    /// Market slug (repeatable, e.g. --slug btc-updown-5m)
    #[clap(long = "slug", action = clap::ArgAction::Append)]
    pub slugs: Vec<String>,

    /// Window size in seconds, paired with each --slug (repeatable)
    #[clap(long = "interval-secs", action = clap::ArgAction::Append)]
    pub intervals: Vec<u64>,

    /// Orderbook depth levels
    #[clap(long)]
    pub depth: Option<usize>,

    /// Terminal render interval in milliseconds
    #[clap(long)]
    pub render_interval: Option<u64>,

    /// Root directory for recorded files (recorder only)
    #[clap(long)]
    pub base_path: Option<String>,

    /// zstd compression level (recorder only)
    #[clap(long)]
    pub zstd_level: Option<u8>,
}

/// Root TOML config for polymarket tools (`configs/polymarket.toml`).
#[derive(Debug, Deserialize, Default)]
pub struct PolymarketTomlConfig {
    #[serde(default)]
    pub server: PolymarketConfig,
    #[serde(default)]
    pub polymarket: Vec<PolymarketMarket>,
}

impl PolymarketTomlConfig {
    pub fn load(path: &str) -> Self {
        let s = std::fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("Failed to read config '{path}': {e}");
            std::process::exit(1);
        });
        toml::from_str(&s).unwrap_or_else(|e| {
            eprintln!("Failed to parse config '{path}': {e}");
            std::process::exit(1);
        })
    }

    /// Apply CLI overrides and inline slug flags, then validate.
    pub fn apply_args(&mut self, args: PolymarketArgs) {
        if let Some(d) = args.depth {
            self.server.depth = d;
        }
        if let Some(r) = args.render_interval {
            self.server.render_interval = r;
        }
        if let Some(p) = args.base_path {
            self.server.base_path = p;
        }
        if let Some(z) = args.zstd_level {
            self.server.zstd_level = z;
        }

        if !args.slugs.is_empty() {
            self.polymarket = args
                .slugs
                .into_iter()
                .enumerate()
                .map(|(i, slug)| PolymarketMarket {
                    enabled: true,
                    slug,
                    interval_secs: *args.intervals.get(i).unwrap_or(&0),
                })
                .collect();
        }
    }
}
