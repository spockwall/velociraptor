use serde::Deserialize;

/// `fetcher:` section — archive output dirs for the two long-running
/// fetcher binaries (`asset_id_fetcher`, `price_to_beat_fetcher`).
///
/// Each binary still accepts a `--archive-dir` CLI flag which, when given,
/// overrides the matching field here. When the flag is omitted the binary
/// uses the config value; the `Default` impl preserves each binary's original
/// built-in path so existing configs without a `fetcher:` section are
/// unaffected.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FetcherConfig {
    /// Archive root for `asset_id_fetcher` (per-day CSVs of
    /// `(market_id, yes_asset_id, no_asset_id)`).
    pub asset_id_dir: String,
    /// Archive root for `price_to_beat_fetcher` (per-day CSVs of
    /// `(price_to_beat, final_price, direction)`).
    pub price_to_beat_dir: String,
}

impl Default for FetcherConfig {
    fn default() -> Self {
        Self {
            asset_id_dir: "./data/asset_ids".to_string(),
            price_to_beat_dir: "./data/price_to_beat".to_string(),
        }
    }
}
