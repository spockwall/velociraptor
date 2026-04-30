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

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use clap::Parser;
use libs::configs::Config;
use orderbook::price_to_beat::{
    REQUEST_SPACING_MS, append_csv, build_row, fetch_kalshi, fetch_polymarket,
    kalshi_ticker_for_window, known_window_starts, latest_window_start,
};
use tracing::{debug, error, info, warn};

const DEFAULT_SEED_FROM: &str = "2026-04-10T00:00:00Z";
const DEFAULT_LOOKBACK_SECS: i64 = 3600; // 1h

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

    #[arg(long, default_value = "./data/price_to_beat")]
    archive_dir: String,
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
    archive_dir: &std::path::Path,
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
    let archive_dir = PathBuf::from(&args.archive_dir);
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
