//! Target price backfill for Polymarket and Kalshi.
//!
//! Walks every window between `--from` and `--to` for one base slug (or one
//! Kalshi series), fetches the upstream public REST API, and appends any
//! missing rows to a daily CSV under
//! `{archive_dir}/{exchange}/{base_slug}/{YYYY-MM-DD}.csv`.
//!
//! Idempotent — rows already present (matched by `window_start`) are skipped.
//!
//! CSV schema:
//!   ts_recorded, exchange, base_slug, full_slug,
//!   window_start, window_end, price_to_beat, final_price, direction
//!
//! Examples:
//!   # Polymarket 15-min market, last 24h
//!   target_price_fetcher polymarket \
//!     --base-slug btc-updown-15m --interval-secs 900 \
//!     --from 2026-04-29T00:00:00Z
//!
//!   # Polymarket 5-min market
//!   target_price_fetcher polymarket \
//!     --base-slug btc-updown-5m --interval-secs 300 \
//!     --from 2026-04-29T00:00:00Z
//!
//!   # Kalshi 15-min series
//!   target_price_fetcher kalshi \
//!     --series KXBTC15M --interval-secs 900 \
//!     --from 2026-04-29T00:00:00Z

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use clap::{Parser, Subcommand};
use libs::endpoints::kalshi::kalshi as kalshi_ep;
use orderbook::exchanges::kalshi::build_market_ticker;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};

#[derive(Parser, Debug)]
#[command(
    name = "target_price_fetcher",
    about = "Backfill Polymarket / Kalshi target prices into per-day CSV files"
)]
struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Backfill Polymarket Up/Down markets.
    Polymarket(PolyArgs),
    /// Backfill Kalshi binary-strike series (e.g. `KXBTC15M`).
    Kalshi(KalshiArgs),
}

#[derive(Parser, Debug, Clone)]
struct PolyArgs {
    /// Base slug, e.g. `btc-updown-15m` or `btc-updown-5m`.
    #[arg(long)]
    base_slug: String,

    /// Window cadence in seconds. 900 = 15 min, 300 = 5 min.
    #[arg(long, default_value_t = 900)]
    interval_secs: i64,

    /// Inclusive start (RFC3339).
    #[arg(long)]
    from: DateTime<Utc>,

    /// Exclusive end. Defaults to now.
    #[arg(long)]
    to: Option<DateTime<Utc>>,

    #[arg(long, default_value_t = 8)]
    http_timeout_secs: u64,

    #[arg(long, default_value = "./data/price_to_beat")]
    archive_dir: String,
}

#[derive(Parser, Debug, Clone)]
struct KalshiArgs {
    /// Series, e.g. `KXBTC15M`.
    #[arg(long)]
    series: String,

    /// Window cadence in seconds. Kalshi's BTC/ETH 15M series uses 900.
    #[arg(long, default_value_t = 900)]
    interval_secs: i64,

    /// Inclusive start (RFC3339).
    #[arg(long)]
    from: DateTime<Utc>,

    /// Exclusive end. Defaults to now.
    #[arg(long)]
    to: Option<DateTime<Utc>>,

    #[arg(long, default_value_t = 8)]
    http_timeout_secs: u64,

    #[arg(long, default_value = "./data/price_to_beat")]
    archive_dir: String,
}

#[derive(Debug, Clone, Default)]
struct Targets {
    line: Option<f64>,
    final_price: Option<f64>,
    end_iso: Option<String>,
    state: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CsvRow {
    ts_recorded: i64,
    exchange: String,
    base_slug: String,
    full_slug: String,
    window_start: i64,
    window_end: i64,
    price_to_beat: String,
    final_price: String,
    direction: String,
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

const REQUEST_SPACING_MS: u64 = 100; // ms

// ── Fetchers ────────────────────────────────────────────────────────────────

async fn fetch_polymarket(http: &reqwest::Client, slug: &str) -> Result<Targets> {
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
    })
}

/// Kalshi binary-strike markets carry the strike in `cap_strike` /
/// `floor_strike`; the resolved settlement price is `settlement_value` (cents)
/// once the market closes.
async fn fetch_kalshi(http: &reqwest::Client, ticker: &str) -> Result<Targets> {
    let url = format!("{}/markets/{}", kalshi_ep::BASE_URL, ticker);
    let resp = http.get(&url).send().await?.error_for_status()?;
    let body: Value = resp.json().await?;
    let market = body.get("market").unwrap_or(&body);

    // The strike (constant per market) is the closest analogue to priceToBeat.
    let line = num(market, "cap_strike")
        .or_else(|| num(market, "floor_strike"))
        .or_else(|| num(market, "strike_value"));
    // `settlement_value` is reported in cents (0–100); other Kalshi numeric
    // resolution fields exist on some product types — use whatever is present.
    let final_price = num(market, "settlement_value").or_else(|| num(market, "result_value"));
    let status = market.get("status").and_then(|v| v.as_str()).unwrap_or("");
    let state = Some(match status {
        "settled" => "resolved",
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
            .map(|s| s.to_string()),
    })
}

// ── CSV layer ───────────────────────────────────────────────────────────────

fn csv_path(root: &Path, exchange: &str, base_slug: &str, day: DateTime<Utc>) -> PathBuf {
    root.join(exchange)
        .join(base_slug)
        .join(format!("{}.csv", day.format("%Y-%m-%d")))
}

fn known_window_starts(root: &Path, exchange: &str, base_slug: &str) -> HashSet<i64> {
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

fn append_csv(root: &Path, row: &CsvRow) -> Result<()> {
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

fn build_row(
    exchange: &str,
    base_slug: &str,
    full_slug: &str,
    window_start: i64,
    interval_secs: i64,
    t: &Targets,
) -> Option<CsvRow> {
    if t.line.is_none() && t.final_price.is_none() {
        return None;
    }
    let window_end = t
        .end_iso
        .as_deref()
        .and_then(iso_to_unix)
        .unwrap_or(window_start + interval_secs);
    Some(CsvRow {
        ts_recorded: Utc::now().timestamp(),
        exchange: exchange.to_string(),
        base_slug: base_slug.to_string(),
        full_slug: full_slug.to_string(),
        window_start,
        window_end,
        price_to_beat: t.line.map(|v| v.to_string()).unwrap_or_default(),
        final_price: t.final_price.map(|v| v.to_string()).unwrap_or_default(),
        direction: direction(t.line, t.final_price).to_string(),
    })
}

// ── Backfill drivers ────────────────────────────────────────────────────────

async fn run_polymarket(args: PolyArgs) -> Result<()> {
    let to = args.to.unwrap_or_else(Utc::now);
    if to <= args.from {
        anyhow::bail!("--to must be after --from");
    }
    if args.interval_secs <= 0 {
        anyhow::bail!("--interval-secs must be positive");
    }
    let archive_dir = PathBuf::from(&args.archive_dir);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(args.http_timeout_secs))
        .user_agent("velociraptor-target-price-fetcher/0.1")
        .build()?;

    let already = known_window_starts(&archive_dir, "polymarket", &args.base_slug);
    info!(
        base_slug = %args.base_slug,
        interval_secs = args.interval_secs,
        already_recorded = already.len(),
        "polymarket backfill: starting"
    );

    let interval = args.interval_secs;
    let from_ts = (args.from.timestamp() / interval) * interval;
    let to_ts = (to.timestamp() / interval) * interval;
    let mut window_start = from_ts;
    let mut fetched = 0usize;
    let mut wrote = 0usize;
    let mut skipped = 0usize;

    while window_start < to_ts {
        if already.contains(&window_start) {
            skipped += 1;
            window_start += interval;
            continue;
        }
        let slug = format!("{}-{}", args.base_slug, window_start);
        match fetch_polymarket(&http, &slug).await {
            Ok(t) => {
                fetched += 1;
                if let Some(row) = build_row(
                    "polymarket",
                    &args.base_slug,
                    &slug,
                    window_start,
                    interval,
                    &t,
                ) {
                    if let Err(e) = append_csv(&archive_dir, &row) {
                        warn!(slug, "csv append failed: {e}");
                    } else {
                        wrote += 1;
                    }
                } else {
                    debug!(slug, state = ?t.state, "no data yet — skipping");
                }
            }
            Err(e) => warn!(slug, "fetch failed: {e}"),
        }

        tokio::time::sleep(Duration::from_millis(REQUEST_SPACING_MS)).await;

        window_start += interval;
    }

    info!(
        fetched,
        wrote,
        skipped_existing = skipped,
        "polymarket backfill: done"
    );
    Ok(())
}

async fn run_kalshi(args: KalshiArgs) -> Result<()> {
    let to = args.to.unwrap_or_else(Utc::now);
    if to <= args.from {
        anyhow::bail!("--to must be after --from");
    }
    if args.interval_secs <= 0 {
        anyhow::bail!("--interval-secs must be positive");
    }
    let archive_dir = PathBuf::from(&args.archive_dir);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(args.http_timeout_secs))
        .user_agent("velociraptor-target-price-fetcher/0.1")
        .build()?;

    let already = known_window_starts(&archive_dir, "kalshi", &args.series);
    info!(
        series = %args.series,
        interval_secs = args.interval_secs,
        already_recorded = already.len(),
        "kalshi backfill: starting"
    );

    let interval = args.interval_secs;
    let from_ts = (args.from.timestamp() / interval) * interval;
    let to_ts = (to.timestamp() / interval) * interval;
    let mut window_start = from_ts;
    let mut fetched = 0usize;
    let mut wrote = 0usize;
    let mut skipped = 0usize;

    while window_start < to_ts {
        if already.contains(&window_start) {
            skipped += 1;
            window_start += interval;
            continue;
        }
        // Kalshi tickers are derived from the *close* time, not the start.
        let close = Utc
            .timestamp_opt(window_start + interval, 0)
            .single()
            .context("bad window_start")?;
        let ticker = build_market_ticker(&args.series, close);

        match fetch_kalshi(&http, &ticker).await {
            Ok(t) => {
                fetched += 1;
                if let Some(row) =
                    build_row("kalshi", &args.series, &ticker, window_start, interval, &t)
                {
                    if let Err(e) = append_csv(&archive_dir, &row) {
                        warn!(ticker, "csv append failed: {e}");
                    } else {
                        wrote += 1;
                    }
                } else {
                    debug!(ticker, state = ?t.state, "no data yet — skipping");
                }
            }
            Err(e) => warn!(ticker, "fetch failed: {e}"),
        }

        tokio::time::sleep(Duration::from_millis(REQUEST_SPACING_MS)).await;
        window_start += interval;
    }

    info!(
        fetched,
        wrote,
        skipped_existing = skipped,
        "kalshi backfill: done"
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    match args.cmd {
        Cmd::Polymarket(a) => run_polymarket(a).await,
        Cmd::Kalshi(a) => run_kalshi(a).await,
    }
}
