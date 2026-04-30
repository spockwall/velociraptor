//! Price-to-beat backfill for Polymarket and Kalshi.
//!
//! Walks every window between `--from` and `--to` for one base slug (or one
//! Kalshi series), fetches the upstream public REST API, and appends any
//! missing rows to a daily CSV under
//! `{archive_dir}/{exchange}/{base_slug}/{YYYY-MM-DD}.csv`.
//!
//! Idempotent — rows already present (matched by `window_start`) are skipped.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use orderbook::price_to_beat::{
    REQUEST_SPACING_MS, append_csv, build_row, fetch_kalshi, fetch_polymarket,
    kalshi_ticker_for_window, known_window_starts,
};
use tracing::{debug, info, warn};

#[derive(Parser, Debug)]
#[command(
    name = "price_to_beat_backfill",
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
        .user_agent("velociraptor-price-to-beat-backfill/0.1")
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
    let (mut fetched, mut wrote, mut skipped) = (0usize, 0usize, 0usize);

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
        .user_agent("velociraptor-price-to-beat-backfill/0.1")
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
    let (mut fetched, mut wrote, mut skipped) = (0usize, 0usize, 0usize);

    while window_start < to_ts {
        if already.contains(&window_start) {
            skipped += 1;
            window_start += interval;
            continue;
        }
        let ticker = kalshi_ticker_for_window(&args.series, window_start, interval)?;
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
