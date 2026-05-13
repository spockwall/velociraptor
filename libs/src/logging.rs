//! Shared tracing setup for Velociraptor binaries.
//!
//! Layout under `base_dir`:
//!
//! ```text
//! {base_dir}/{service}/{YYYY-MM-DD}.log         — INFO+ (everything filtered by env_filter)
//! {base_dir}/{service}/{YYYY-MM-DD}.error.log   — WARN+ only
//! ```
//!
//! Files rotate daily via `tracing_appender::rolling`. The returned
//! `LogGuards` must be held in `main` for the lifetime of the program so the
//! non-blocking writers can flush on shutdown.
//!
//! In addition to file output, logs are mirrored to stdout (for `docker logs`
//! / interactive runs).

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Worker guards for the non-blocking file appenders. Drop = flush.
/// Bind in `main`; do not let it go out of scope until the process exits.
pub struct LogGuards {
    _info: WorkerGuard,
    _error: WorkerGuard,
    _stdout: WorkerGuard,
}

/// Initialise tracing for a binary.
///
/// - `service`: subdirectory name under `base_dir` (e.g. `"server"`, `"executor"`).
/// - `base_dir`: root log directory (e.g. `/app/syslog`).
/// - `filter`: env-filter string for stdout + the main `.log` file (e.g. `"info"`).
/// - `json`: emit JSON instead of human-readable text on stdout + files.
///
/// Returns guards that flush the async writers on drop.
pub fn init_logging(service: &str, base_dir: &Path, filter: &str, json: bool) -> LogGuards {
    let dir = base_dir.join(service);
    std::fs::create_dir_all(&dir).expect("create log directory");

    // Daily-rotating appenders. `rolling::daily(dir, prefix)` writes
    // `{prefix}.{YYYY-MM-DD}`; pre-add the suffix we want in the name.
    let info_file = rolling::daily(&dir, "log");
    let error_file = rolling::daily(&dir, "error.log");

    let (info_writer, info_guard) = tracing_appender::non_blocking(info_file);
    let (error_writer, error_guard) = tracing_appender::non_blocking(error_file);
    let (stdout_writer, stdout_guard) = tracing_appender::non_blocking(std::io::stdout());

    let env_filter = || EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));

    let registry = tracing_subscriber::registry();

    if json {
        let info_layer = fmt::layer()
            .json()
            .with_writer(info_writer)
            .with_ansi(false)
            .with_filter(env_filter());
        let error_layer = fmt::layer()
            .json()
            .with_writer(error_writer)
            .with_ansi(false)
            .with_filter(LevelFilter::WARN);
        let stdout_layer = fmt::layer()
            .json()
            .with_writer(stdout_writer)
            .with_filter(env_filter());
        registry
            .with(info_layer)
            .with(error_layer)
            .with(stdout_layer)
            .init();
    } else {
        let info_layer = fmt::layer()
            .with_writer(info_writer)
            .with_ansi(false)
            .with_filter(env_filter());
        let error_layer = fmt::layer()
            .with_writer(error_writer)
            .with_ansi(false)
            .with_filter(LevelFilter::WARN);
        let stdout_layer = fmt::layer()
            .with_writer(stdout_writer)
            .with_filter(env_filter());
        registry
            .with(info_layer)
            .with(error_layer)
            .with(stdout_layer)
            .init();
    }

    LogGuards {
        _info: info_guard,
        _error: error_guard,
        _stdout: stdout_guard,
    }
}
