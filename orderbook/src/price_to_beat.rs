//! Shared logic for the `price_to_beat_backfill` and `price_to_beat_fetcher`
//! binaries: fetching, CSV layout, dedup, row construction.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use libs::endpoints::kalshi::kalshi as kalshi_ep;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::exchanges::kalshi::build_market_ticker;

/// Inter-request spacing (ms) for both backfill and live fetcher loops.
pub const REQUEST_SPACING_MS: u64 = 100;

/// Resolved target+outcome for a single window.
#[derive(Debug, Clone, Default)]
pub struct Targets {
    pub line: Option<f64>,
    pub final_price: Option<f64>,
    pub end_iso: Option<String>,
    pub state: Option<&'static str>,
    /// Pre-computed direction. Kalshi sets this from `result`; Polymarket
    /// leaves it `None` and lets [`direction`] compute from `(line, final_price)`.
    pub direction_override: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsvRow {
    pub ts_recorded: i64,
    pub exchange: String,
    pub base_slug: String,
    pub full_slug: String,
    pub window_start: i64,
    pub window_end: i64,
    pub price_to_beat: String,
    pub final_price: String,
    pub direction: String,
}

fn num(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

fn iso_to_unix(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp())
}

// ── Fetchers ────────────────────────────────────────────────────────────────

pub async fn fetch_polymarket(http: &reqwest::Client, slug: &str) -> Result<Targets> {
    let url = format!("https://gamma-api.polymarket.com/markets/slug/{slug}");
    let resp = http.get(&url).send().await?.error_for_status()?;
    let body: Value = resp.json().await?;

    let event_md = body
        .get("events")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|e| e.get("eventMetadata"));

    let line = event_md
        .and_then(|md| num(md, "priceToBeat"))
        .or_else(|| num(&body, "line"));
    let final_price = event_md.and_then(|md| num(md, "finalPrice"));

    let closed = body
        .get("closed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let accepting = body
        .get("acceptingOrders")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let state = Some(match (closed, final_price.is_some()) {
        (true, true) => "resolved",
        (true, false) => "closed",
        (false, _) if accepting => "open",
        _ => "pending",
    });

    Ok(Targets {
        line,
        final_price,
        state,
        end_iso: body
            .get("endDate")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        direction_override: None,
    })
}

/// See `price_to_beat_backfill.rs` for the field-by-field semantics.
pub async fn fetch_kalshi(http: &reqwest::Client, ticker: &str) -> Result<Targets> {
    let url = format!("{}/markets/{}", kalshi_ep::BASE_URL, ticker);
    let resp = http.get(&url).send().await?.error_for_status()?;
    let body: Value = resp.json().await?;
    let market = body.get("market").unwrap_or(&body);

    let line = num(market, "floor_strike")
        .or_else(|| num(market, "cap_strike"))
        .or_else(|| num(market, "strike_value"));

    let final_price = num(market, "expiration_value");

    let result = market.get("result").and_then(|v| v.as_str());
    let direction_override = match result {
        Some("yes") => Some("up"),
        Some("no") => Some("down"),
        _ => None,
    };

    let status = market.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let state = Some(match status {
        "finalized" | "settled" => "resolved",
        "closed" => "closed",
        "open" | "active" => "open",
        _ => "pending",
    });

    Ok(Targets {
        line,
        final_price,
        state,
        end_iso: market
            .get("close_time")
            .and_then(|v| v.as_str())
            .or_else(|| {
                market
                    .get("expected_expiration_time")
                    .and_then(|v| v.as_str())
            })
            .map(|s| s.to_string()),
        direction_override,
    })
}

// ── CSV layer ───────────────────────────────────────────────────────────────

pub fn csv_path(root: &Path, exchange: &str, base_slug: &str, day: DateTime<Utc>) -> PathBuf {
    root.join(exchange)
        .join(base_slug)
        .join(format!("{}.csv", day.format("%Y-%m-%d")))
}

/// Walk every CSV under `{root}/{exchange}/{base_slug}/` and return the set
/// of `window_start` timestamps already on disk. Used for dedup.
pub fn known_window_starts(root: &Path, exchange: &str, base_slug: &str) -> HashSet<i64> {
    let dir = root.join(exchange).join(base_slug);
    let mut seen = HashSet::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return seen;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("csv") {
            continue;
        }
        let Ok(mut rdr) = csv::Reader::from_path(&p) else {
            continue;
        };
        for row in rdr.deserialize::<CsvRow>().flatten() {
            seen.insert(row.window_start);
        }
    }
    seen
}

/// Largest `window_start` already on disk for this market, or `None` if
/// nothing has been recorded yet.
pub fn latest_window_start(root: &Path, exchange: &str, base_slug: &str) -> Option<i64> {
    known_window_starts(root, exchange, base_slug)
        .into_iter()
        .max()
}

pub fn append_csv(root: &Path, row: &CsvRow) -> Result<()> {
    let day =
        DateTime::<Utc>::from_timestamp(row.window_start, 0).context("invalid window_start")?;
    let path = csv_path(root, &row.exchange, &row.base_slug, day);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("mkdir csv parent")?;
    }
    let new_file = !path.exists();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .context("open csv")?;
    let mut wtr = csv::WriterBuilder::new()
        .has_headers(new_file)
        .from_writer(file);
    wtr.serialize(row).context("serialize row")?;
    wtr.flush().context("flush csv")?;
    Ok(())
}

fn direction(price_to_beat: Option<f64>, final_price: Option<f64>) -> &'static str {
    match (price_to_beat, final_price) {
        (Some(start), Some(end)) if end >= start => "up",
        (Some(_), Some(_)) => "down",
        _ => "",
    }
}

pub fn build_row(
    exchange: &str,
    base_slug: &str,
    full_slug: &str,
    window_start: i64,
    interval_secs: i64,
    t: &Targets,
) -> Option<CsvRow> {
    let kalshi_has_outcome = t.direction_override.is_some();
    if t.line.is_none() && t.final_price.is_none() && !kalshi_has_outcome {
        return None;
    }
    let window_end = t
        .end_iso
        .as_deref()
        .and_then(iso_to_unix)
        .unwrap_or(window_start + interval_secs);
    let dir = t
        .direction_override
        .unwrap_or_else(|| direction(t.line, t.final_price));
    Some(CsvRow {
        ts_recorded: Utc::now().timestamp(),
        exchange: exchange.to_string(),
        base_slug: base_slug.to_string(),
        full_slug: full_slug.to_string(),
        window_start,
        window_end,
        price_to_beat: t.line.map(|v| v.to_string()).unwrap_or_default(),
        final_price: t.final_price.map(|v| v.to_string()).unwrap_or_default(),
        direction: dir.to_string(),
    })
}

/// Build the Kalshi market ticker for a window starting at `window_start`
/// (Kalshi tickers encode the *close* time, not the start).
pub fn kalshi_ticker_for_window(
    series: &str,
    window_start: i64,
    interval_secs: i64,
) -> Result<String> {
    let close = Utc
        .timestamp_opt(window_start + interval_secs, 0)
        .single()
        .context("bad window_start")?;
    Ok(build_market_ticker(series, close))
}
