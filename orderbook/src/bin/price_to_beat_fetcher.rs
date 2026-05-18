//! Long-running price-to-beat fetcher for Polymarket and Kalshi.
//!
//! Reads `configs/example.yaml` and, for every enabled market, drives a
//! per-market loop that:
//!
//! 1. **Auto-backfills on startup**: walks every window between
//!    `max(latest_csv_window + interval, --seed-from)` and `now - lookback`
//!    and fetches any missing rows.
//! 2. **Polls forever** at the market's cadence: each tick targets the window
//!    that closed `--lookback-secs` ago (default 1 h) so `priceToBeat` /
//!    `expiration_value` are reliably populated.
//!
//! Idempotent — the on-disk CSV is the source of truth; restarting fills any
//! gaps automatically.
//!
//! Run:
//!   cargo run --bin price_to_beat_fetcher --release -- --config configs/example.yaml

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use clap::Parser;
use libs::configs::Config;
use libs::endpoints::kalshi::kalshi as kalshi_ep;
use orderbook::exchanges::kalshi::build_market_ticker;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, error, info, warn};

const DEFAULT_SEED_FROM: &str = "2026-04-10T00:00:00Z";
const DEFAULT_LOOKBACK_SECS: i64 = 3600; // 1h

/// Inter-request spacing (ms) for the per-market fetch loop.
const REQUEST_SPACING_MS: u64 = 100;

// ── Shared types ────────────────────────────────────────────────────────────

/// Resolved target+outcome for a single window.
#[derive(Debug, Clone, Default)]
struct Targets {
    line: Option<f64>,
    final_price: Option<f64>,
    end_iso: Option<String>,
    state: Option<&'static str>,
    /// Pre-computed direction. Kalshi sets this from `result`; Polymarket
    /// leaves it `None` and lets `direction` compute from `(line, final_price)`.
    direction_override: Option<&'static str>,
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
        direction_override: None,
    })
}

async fn fetch_kalshi(http: &reqwest::Client, ticker: &str) -> Result<Targets> {
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

fn csv_path(root: &Path, exchange: &str, base_slug: &str, day: DateTime<Utc>) -> PathBuf {
    root.join(exchange)
        .join(base_slug)
        .join(format!("{}.csv", day.format("%Y-%m-%d")))
}

/// Walk every CSV under `{root}/{exchange}/{base_slug}/` and return the set
/// of `window_start` timestamps already on disk. Used for dedup.
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

/// Largest `window_start` already on disk for this market, or `None` if
/// nothing has been recorded yet.
fn latest_window_start(root: &Path, exchange: &str, base_slug: &str) -> Option<i64> {
    known_window_starts(root, exchange, base_slug)
        .into_iter()
        .max()
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
fn kalshi_ticker_for_window(series: &str, window_start: i64, interval_secs: i64) -> Result<String> {
    let close = Utc
        .timestamp_opt(window_start + interval_secs, 0)
        .single()
        .context("bad window_start")?;
    Ok(build_market_ticker(series, close))
}

// ── CLI ─────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "price_to_beat_fetcher",
    about = "Continuously fetch Polymarket / Kalshi target prices into per-day CSV files"
)]
struct Args {
    #[arg(long, default_value = "configs/example.yaml")]
    config: String,

    /// How far back the targeted window should have closed before we fetch
    /// it. Default 1h; both Polymarket `priceToBeat`/`finalPrice` and Kalshi
    /// `expiration_value`/`result` are reliably present by then.
    #[arg(long, default_value_t = DEFAULT_LOOKBACK_SECS)]
    lookback_secs: i64,

    /// Seed start date for the auto-backfill on a fresh archive. If at least
    /// one row already exists for a market, the backfill resumes from
    /// `latest_csv_window + interval` instead.
    #[arg(long, default_value = DEFAULT_SEED_FROM)]
    seed_from: DateTime<Utc>,

    #[arg(long, default_value_t = 8)]
    http_timeout_secs: u64,

    /// Archive root. Overrides `fetcher.price_to_beat_dir` from the config
    /// when given; otherwise the config value is used.
    #[arg(long)]
    archive_dir: Option<String>,
}

#[derive(Debug, Clone)]
enum Job {
    Polymarket {
        base_slug: String,
        interval_secs: i64,
    },
    Kalshi {
        series: String,
        interval_secs: i64,
    },
}

impl Job {
    fn exchange(&self) -> &'static str {
        match self {
            Job::Polymarket { .. } => "polymarket",
            Job::Kalshi { .. } => "kalshi",
        }
    }
    fn ident(&self) -> &str {
        match self {
            Job::Polymarket { base_slug, .. } => base_slug,
            Job::Kalshi { series, .. } => series,
        }
    }
    fn interval_secs(&self) -> i64 {
        match self {
            Job::Polymarket { interval_secs, .. } | Job::Kalshi { interval_secs, .. } => {
                *interval_secs
            }
        }
    }
}

/// Fetch and persist one window. Returns `Ok(true)` if a row was written,
/// `Ok(false)` on a successful fetch with no recordable data (e.g. window
/// not yet resolved), or `Err` on transport failure.
async fn fetch_one_window(
    http: &reqwest::Client,
    archive_dir: &Path,
    job: &Job,
    window_start: i64,
) -> Result<bool> {
    let interval = job.interval_secs();
    match job {
        Job::Polymarket { base_slug, .. } => {
            let slug = format!("{base_slug}-{window_start}");
            let t = fetch_polymarket(http, &slug).await?;
            if let Some(row) = build_row("polymarket", base_slug, &slug, window_start, interval, &t)
            {
                append_csv(archive_dir, &row)?;
                Ok(true)
            } else {
                debug!(slug, state = ?t.state, "polymarket: no data yet");
                Ok(false)
            }
        }
        Job::Kalshi { series, .. } => {
            let ticker = kalshi_ticker_for_window(series, window_start, interval)?;
            let t = fetch_kalshi(http, &ticker).await?;
            if let Some(row) = build_row("kalshi", series, &ticker, window_start, interval, &t) {
                append_csv(archive_dir, &row)?;
                Ok(true)
            } else {
                debug!(ticker, state = ?t.state, "kalshi: no data yet");
                Ok(false)
            }
        }
    }
}

/// Drive one market: auto-backfill from the gap, then poll forever.
async fn run_job(
    job: Job,
    http: reqwest::Client,
    archive_dir: PathBuf,
    seed_from_ts: i64,
    lookback_secs: i64,
) {
    let interval = job.interval_secs();
    let exchange = job.exchange();
    let ident = job.ident().to_string();

    // ── 1) Auto-backfill ────────────────────────────────────────────────────
    let resume_from = match latest_window_start(&archive_dir, exchange, &ident) {
        Some(latest) => latest + interval,
        None => (seed_from_ts / interval) * interval,
    };
    let cutoff = ((Utc::now().timestamp() - lookback_secs) / interval) * interval;

    if resume_from < cutoff {
        info!(
            exchange,
            ident,
            interval,
            from = resume_from,
            to = cutoff,
            count = (cutoff - resume_from) / interval,
            "auto-backfill: starting"
        );
        let already = known_window_starts(&archive_dir, exchange, &ident);
        let mut window_start = resume_from;
        let (mut wrote, mut skipped, mut errs) = (0usize, 0usize, 0usize);
        while window_start < cutoff {
            if already.contains(&window_start) {
                skipped += 1;
                window_start += interval;
                continue;
            }
            match fetch_one_window(&http, &archive_dir, &job, window_start).await {
                Ok(true) => wrote += 1,
                Ok(false) => {}
                Err(e) => {
                    warn!(exchange, ident, window_start, "backfill fetch failed: {e}");
                    errs += 1;
                }
            }
            tokio::time::sleep(Duration::from_millis(REQUEST_SPACING_MS)).await;
            window_start += interval;
        }
        info!(exchange, ident, wrote, skipped, errs, "auto-backfill: done");
    } else {
        info!(exchange, ident, "auto-backfill: nothing to do");
    }

    // ── 2) Live loop ────────────────────────────────────────────────────────
    info!(exchange, ident, interval, lookback_secs, "live: started");
    loop {
        // Sleep to the next interval boundary so we tick aligned with the
        // exchange's window roll.
        let now_s = Utc::now().timestamp();
        let next = ((now_s / interval) + 1) * interval;
        let sleep_for = (next - now_s).max(1) as u64;
        tokio::time::sleep(Duration::from_secs(sleep_for)).await;

        // Target the window that closed `lookback_secs` ago.
        let target = Utc::now().timestamp() - lookback_secs;
        let window_start = (target / interval) * interval;

        // Skip if already on disk (e.g. backfill caught up to here).
        if known_window_starts(&archive_dir, exchange, &ident).contains(&window_start) {
            continue;
        }

        match fetch_one_window(&http, &archive_dir, &job, window_start).await {
            Ok(true) => info!(exchange, ident, window_start, "wrote row"),
            Ok(false) => debug!(exchange, ident, window_start, "no row yet"),
            Err(e) => warn!(exchange, ident, window_start, "fetch failed: {e}"),
        }
    }
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
    let cfg = Config::load(&args.config);
    let archive_dir = PathBuf::from(
        args.archive_dir
            .clone()
            .unwrap_or_else(|| cfg.fetcher.price_to_beat_dir.clone()),
    );
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(args.http_timeout_secs))
        .user_agent("velociraptor-price-to-beat-fetcher/0.1")
        .build()?;

    let mut jobs: Vec<Job> = Vec::new();
    for m in cfg.polymarket.markets.iter().filter(|m| m.enabled) {
        if m.interval_secs == 0 {
            warn!(slug = m.slug, "skipping static polymarket market");
            continue;
        }
        jobs.push(Job::Polymarket {
            base_slug: m.slug.clone(),
            interval_secs: m.interval_secs as i64,
        });
    }
    for m in cfg.kalshi.market.iter().filter(|m| m.enable) {
        if m.interval_secs == 0 {
            warn!(series = m.series, "skipping static kalshi market");
            continue;
        }
        jobs.push(Job::Kalshi {
            series: m.series.clone(),
            interval_secs: m.interval_secs as i64,
        });
    }

    if jobs.is_empty() {
        error!("no enabled markets in config — nothing to fetch");
        std::process::exit(1);
    }

    // Group log line: which markets we're tracking.
    let by_interval: HashMap<i64, Vec<String>> = jobs.iter().fold(HashMap::new(), |mut acc, j| {
        acc.entry(j.interval_secs())
            .or_default()
            .push(format!("{}:{}", j.exchange(), j.ident()));
        acc
    });
    info!(
        markets = jobs.len(),
        intervals = ?by_interval,
        seed_from = %args.seed_from,
        lookback_secs = args.lookback_secs,
        "price_to_beat_fetcher: starting"
    );

    let seed_from_ts = args.seed_from.timestamp();

    let mut handles = Vec::new();
    for job in jobs {
        let http = http.clone();
        let archive_dir = archive_dir.clone();
        handles.push(tokio::spawn(async move {
            run_job(job, http, archive_dir, seed_from_ts, args.lookback_secs).await
        }));
    }
    for h in handles {
        let _ = h.await;
    }
    Ok(())
}
