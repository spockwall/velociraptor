//! Long-running Polymarket `(market_id, yes_asset_id, no_asset_id)` fetcher.
//!
//! Reads `configs/example.yaml` and, for every enabled Polymarket market,
//! drives a per-market loop that:
//!
//! 1. **Auto-backfills on startup**: walks every window between
//!    `max(latest_csv_window + interval, --seed-from)` and the current
//!    window start, fetching the Gamma market record for each slug and
//!    appending any missing rows.
//! 2. **Polls forever** at the market's cadence: each tick targets the
//!    window that just opened. Most exchanges publish `clobTokenIds` and
//!    the market `id` as soon as the slug exists, well before
//!    `priceToBeat` lands, so we fetch immediately on each boundary.
//!
//! Idempotent — the on-disk CSV is the source of truth; restarting fills
//! any gaps automatically.
//!
//! CSV layout: `data/asset_ids/polymarket/{base_slug}/{YYYY-MM-DD}.csv`
//!
//! Columns: ts_recorded, base_slug, full_slug, window_start, window_end,
//!          market_id, yes_asset_id, no_asset_id
//!
//! Kalshi markets are skipped — Kalshi binary markets resolve to a single
//! ticker, not a yes/no token pair, so there's nothing analogous to record.
//!
//! Run:
//!   cargo run --bin asset_id_fetcher --release -- --config configs/example.yaml

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use libs::configs::Config;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, error, info, warn};

const DEFAULT_SEED_FROM: &str = "2026-04-10T00:00:00Z";
const REQUEST_SPACING_MS: u64 = 100;

#[derive(Parser, Debug)]
#[command(
    name = "asset_id_fetcher",
    about = "Continuously fetch Polymarket market_id + yes/no asset IDs into per-day CSV files"
)]
struct Args {
    #[arg(long, default_value = "configs/example.yaml")]
    config: String,

    /// Seed start date for the auto-backfill on a fresh archive. If at least
    /// one row already exists for a market, the backfill resumes from
    /// `latest_csv_window + interval` instead.
    #[arg(long, default_value = DEFAULT_SEED_FROM)]
    seed_from: DateTime<Utc>,

    #[arg(long, default_value_t = 8)]
    http_timeout_secs: u64,

    #[arg(long, default_value = "./data/asset_ids")]
    archive_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CsvRow {
    ts_recorded: i64,
    base_slug: String,
    full_slug: String,
    window_start: i64,
    window_end: i64,
    market_id: String,
    yes_asset_id: String,
    no_asset_id: String,
}

#[derive(Debug, Clone)]
struct Resolved {
    market_id: String,
    yes_asset_id: String,
    no_asset_id: String,
    end_iso: Option<String>,
}

// ── Fetcher ─────────────────────────────────────────────────────────────────

/// Pull `(market_id, clobTokenIds[0], clobTokenIds[1])` from Gamma. Returns
/// `Ok(None)` if the slug exists but lacks token IDs (window not yet
/// initialised); `Err` only on transport / parse failures.
async fn fetch_polymarket_ids(http: &reqwest::Client, slug: &str) -> Result<Option<Resolved>> {
    let url = format!("https://gamma-api.polymarket.com/markets/slug/{slug}");
    let resp = http.get(&url).send().await?;
    if resp.status().as_u16() == 404 {
        return Ok(None);
    }
    let resp = resp.error_for_status()?;
    let body: Value = resp.json().await?;

    // `id` is the integer market id. Stringify so the CSV stays text.
    let market_id = match body.get("id") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => return Ok(None),
    };

    // `clobTokenIds` is a JSON-encoded string `"[yes, no]"`. Outcomes order
    // is `["Up","Down"]` — index 0 = yes/up, index 1 = no/down.
    let raw = match body.get("clobTokenIds") {
        Some(Value::String(s)) => s.clone(),
        _ => return Ok(None),
    };
    let ids: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
    if ids.len() < 2 {
        return Ok(None);
    }

    let end_iso = body
        .get("endDate")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(Some(Resolved {
        market_id,
        yes_asset_id: ids[0].clone(),
        no_asset_id: ids[1].clone(),
        end_iso,
    }))
}

// ── CSV layer ───────────────────────────────────────────────────────────────

fn csv_path(root: &Path, base_slug: &str, day: DateTime<Utc>) -> PathBuf {
    root.join("polymarket")
        .join(base_slug)
        .join(format!("{}.csv", day.format("%Y-%m-%d")))
}

fn known_window_starts(root: &Path, base_slug: &str) -> HashSet<i64> {
    let dir = root.join("polymarket").join(base_slug);
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

fn latest_window_start(root: &Path, base_slug: &str) -> Option<i64> {
    known_window_starts(root, base_slug).into_iter().max()
}

fn append_csv(root: &Path, row: &CsvRow) -> Result<()> {
    let day =
        DateTime::<Utc>::from_timestamp(row.window_start, 0).context("invalid window_start")?;
    let path = csv_path(root, &row.base_slug, day);
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

fn iso_to_unix(s: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp())
}

fn build_row(
    base_slug: &str,
    full_slug: &str,
    window_start: i64,
    interval_secs: i64,
    r: &Resolved,
) -> CsvRow {
    let window_end = r
        .end_iso
        .as_deref()
        .and_then(iso_to_unix)
        .unwrap_or(window_start + interval_secs);
    CsvRow {
        ts_recorded: Utc::now().timestamp(),
        base_slug: base_slug.to_string(),
        full_slug: full_slug.to_string(),
        window_start,
        window_end,
        market_id: r.market_id.clone(),
        yes_asset_id: r.yes_asset_id.clone(),
        no_asset_id: r.no_asset_id.clone(),
    }
}

// ── Per-window operation ────────────────────────────────────────────────────

/// Fetch + persist one window. Returns `Ok(true)` if a row was written,
/// `Ok(false)` on a successful fetch with no resolvable token IDs (slug
/// not yet open), or `Err` on transport / parse failure.
async fn fetch_one_window(
    http: &reqwest::Client,
    archive_dir: &Path,
    base_slug: &str,
    interval_secs: i64,
    window_start: i64,
) -> Result<bool> {
    let slug = format!("{base_slug}-{window_start}");
    match fetch_polymarket_ids(http, &slug).await? {
        Some(r) => {
            let row = build_row(base_slug, &slug, window_start, interval_secs, &r);
            append_csv(archive_dir, &row)?;
            Ok(true)
        }
        None => {
            debug!(slug, "no token ids yet");
            Ok(false)
        }
    }
}

// ── Per-market driver ───────────────────────────────────────────────────────

async fn run_job(
    base_slug: String,
    interval_secs: i64,
    http: reqwest::Client,
    archive_dir: PathBuf,
    seed_from_ts: i64,
) {
    // ── 1) Auto-backfill ────────────────────────────────────────────────────
    let resume_from = match latest_window_start(&archive_dir, &base_slug) {
        Some(latest) => latest + interval_secs,
        None => (seed_from_ts / interval_secs) * interval_secs,
    };
    // Asset IDs land at window-open, so the cutoff is the *current* window
    // start (not `now - lookback` like the price-to-beat fetcher uses).
    let cutoff = (Utc::now().timestamp() / interval_secs) * interval_secs;

    if resume_from < cutoff {
        info!(
            base_slug = %base_slug,
            interval_secs,
            from = resume_from,
            to = cutoff,
            count = (cutoff - resume_from) / interval_secs,
            "auto-backfill: starting"
        );
        let already = known_window_starts(&archive_dir, &base_slug);
        let mut window_start = resume_from;
        let (mut wrote, mut skipped, mut errs) = (0usize, 0usize, 0usize);
        while window_start < cutoff {
            if already.contains(&window_start) {
                skipped += 1;
                window_start += interval_secs;
                continue;
            }
            match fetch_one_window(&http, &archive_dir, &base_slug, interval_secs, window_start)
                .await
            {
                Ok(true) => wrote += 1,
                Ok(false) => {}
                Err(e) => {
                    warn!(base_slug = %base_slug, window_start, "backfill fetch failed: {e}");
                    errs += 1;
                }
            }
            tokio::time::sleep(Duration::from_millis(REQUEST_SPACING_MS)).await;
            window_start += interval_secs;
        }
        info!(base_slug = %base_slug, wrote, skipped, errs, "auto-backfill: done");
    } else {
        info!(base_slug = %base_slug, "auto-backfill: nothing to do");
    }

    // ── 2) Live loop ────────────────────────────────────────────────────────
    info!(base_slug = %base_slug, interval_secs, "live: started");
    loop {
        // Sleep to the next interval boundary, then immediately fetch the
        // *just-opened* window (asset IDs are published at window-open).
        let now_s = Utc::now().timestamp();
        let next = ((now_s / interval_secs) + 1) * interval_secs;
        let sleep_for = (next - now_s).max(1) as u64;
        tokio::time::sleep(Duration::from_secs(sleep_for)).await;

        let window_start = (Utc::now().timestamp() / interval_secs) * interval_secs;

        if known_window_starts(&archive_dir, &base_slug).contains(&window_start) {
            continue;
        }

        match fetch_one_window(&http, &archive_dir, &base_slug, interval_secs, window_start).await {
            Ok(true) => info!(base_slug = %base_slug, window_start, "wrote row"),
            Ok(false) => debug!(base_slug = %base_slug, window_start, "no row yet"),
            Err(e) => warn!(base_slug = %base_slug, window_start, "fetch failed: {e}"),
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
    let archive_dir = PathBuf::from(&args.archive_dir);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(args.http_timeout_secs))
        .user_agent("velociraptor-asset-id-fetcher/0.1")
        .build()?;

    let mut jobs: Vec<(String, i64)> = Vec::new();
    for m in cfg.polymarket.markets.iter().filter(|m| m.enabled) {
        if m.interval_secs == 0 {
            warn!(slug = m.slug, "skipping static polymarket market");
            continue;
        }
        jobs.push((m.slug.clone(), m.interval_secs as i64));
    }

    if jobs.is_empty() {
        error!("no enabled polymarket markets in config — nothing to fetch");
        std::process::exit(1);
    }

    info!(
        markets = jobs.len(),
        seed_from = %args.seed_from,
        archive_dir = %archive_dir.display(),
        "asset_id_fetcher: starting"
    );

    let seed_from_ts = args.seed_from.timestamp();

    let mut handles = Vec::new();
    for (base_slug, interval_secs) in jobs {
        let http = http.clone();
        let archive_dir = archive_dir.clone();
        handles.push(tokio::spawn(async move {
            run_job(base_slug, interval_secs, http, archive_dir, seed_from_ts).await
        }));
    }
    for h in handles {
        let _ = h.await;
    }
    Ok(())
}
