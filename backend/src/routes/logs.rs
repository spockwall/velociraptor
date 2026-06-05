//! Error-log tailer + endpoint.
//!
//!   - `GET /api/logs/errors?limit=N` → recent error-log entries, newest first.
//!
//! ## Where errors come from
//! Each velociraptor binary writes a daily-rotating `…/{service}/{YYYY-MM-DD}.error.log`
//! (WARN+ only) via `libs::logging::init_logging`. The frontend has no shell
//! access to the box, so the backend tails those files and republishes new
//! lines through Redis.
//!
//! ## Tailer + cached cursor
//! [`tail_loop`] is a background task (spawned by `bin/backend.rs`). Every
//! [`TAIL_INTERVAL`], for each watched service it:
//!   1. opens today's `{dir}/{service}/{day}.error.log`,
//!   2. seeks to the **cached cursor** for that service (a `(date, byte_offset)`
//!      persisted in the Redis hash [`System::ERROR_LOG_CURSORS`]),
//!   3. reads everything appended since, parses each line into a [`LogEntry`],
//!      LPUSHes them onto the capped list [`System::ERROR_LOGS`],
//!   4. writes the advanced cursor back.
//!
//! Caching the cursor in Redis (not just in memory) means a backend restart
//! resumes where it left off rather than re-emitting the whole file, and the
//! cursor's *date* component is what makes daily rotation correct: when the day
//! rolls over, the service's file name changes, the cached date no longer
//! matches today, so we start the new file at offset 0 (see [`Cursor`]).
//!
//! `GET /api/logs/errors` just reads the capped list back — cheap, and decoupled
//! from how many frontend clients poll (they don't consume the cursor).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::response::Json;
use libs::redis_client::keys::System as SystemKey;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

/// How often the tailer polls each service's error log for new lines.
pub const TAIL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Max error entries retained in the Redis list. Older entries fall off.
const HISTORY_CAP: usize = 2000;

/// Default number of entries returned by `GET /api/logs/errors`.
const HISTORY_DEFAULT_LIMIT: usize = 200;

/// Cap on bytes read from one file in a single tail pass, so a sudden burst of
/// error output can't make one tick allocate unboundedly. The remainder is
/// picked up on the next tick (the cursor only advances by what we consumed).
const MAX_READ_PER_PASS: u64 = 1024 * 1024;

/// Services whose `.error.log` the tailer watches. These are the subdirectory
/// names passed to `libs::logging::init_logging` by each binary. Kept in sync
/// by hand — adding a binary that should surface errors means adding it here.
const SERVICES: &[&str] = &[
    "backend",
    "server",
    "executor",
    "polymarket_recorder",
    "orderbook_recorder",
    "price_to_beat_fetcher",
    "asset_id_fetcher",
];

/// One parsed error-log line.
///
/// The fields after `raw` are a best-effort parse of tracing's default text
/// format (`<rfc3339 ts>  LEVEL target: message`). When a line doesn't match
/// (custom format, continuation line, JSON), `raw` always holds the original
/// text so nothing is lost, and the structured fields are filled in as far as
/// they parse.
#[derive(Serialize, Deserialize, Clone)]
pub struct LogEntry {
    /// Which service's `.error.log` this came from.
    pub service: String,
    /// Parsed timestamp (RFC3339) if the line started with one, else `null`.
    pub ts: Option<String>,
    /// Parsed level (`WARN` / `ERROR`) if present, else `null`.
    pub level: Option<String>,
    /// Parsed tracing target (e.g. `backend::routes`) if present, else `null`.
    pub target: Option<String>,
    /// The full original log line (always present).
    pub raw: String,
}

/// The tailer's per-service position in the daily error log.
struct Cursor {
    /// `YYYY-MM-DD` of the file the offset refers to.
    date: String,
    /// Byte offset already consumed within that day's file.
    offset: u64,
}

impl Cursor {
    /// Encode as the Redis hash value `"{date}:{offset}"`.
    fn encode(&self) -> String {
        format!("{}:{}", self.date, self.offset)
    }

    /// Parse `"{date}:{offset}"`. A malformed value is treated as "no cursor"
    /// (caller falls back to today @ 0).
    fn parse(s: &str) -> Option<Self> {
        let (date, off) = s.rsplit_once(':')?;
        Some(Cursor {
            date: date.to_string(),
            offset: off.parse().ok()?,
        })
    }
}

// ── HTTP handler ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct LogQuery {
    /// How many entries to return (newest first). Clamped to `HISTORY_CAP`.
    pub limit: Option<usize>,
}

/// Read the capped error-log list written by [`tail_loop`]. Newest first.
pub(crate) async fn get_error_logs(
    State(s): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<LogQuery>,
) -> Result<Json<Vec<LogEntry>>, ApiError> {
    let limit = q.limit.unwrap_or(HISTORY_DEFAULT_LIMIT).clamp(1, HISTORY_CAP);
    let raw = s
        .redis
        .lrange_raw(SystemKey::ERROR_LOGS, 0, limit as isize - 1)
        .await;

    let mut entries = Vec::with_capacity(raw.len());
    for blob in &raw {
        match rmp_serde::from_slice::<LogEntry>(blob) {
            Ok(e) => entries.push(e),
            Err(e) => tracing::warn!("error-log read: skipping undecodable entry: {e}"),
        }
    }
    Ok(Json(entries))
}

// ── Background tailer ────────────────────────────────────────────────────────

/// Tail every watched service's daily `.error.log`, republishing new lines onto
/// the capped Redis list. Spawned once by `bin/backend.rs`; loops until exit.
///
/// `base_dir` is `cfg.logging.dir`; per-service files live under
/// `{base_dir}/{service}/{YYYY-MM-DD}.error.log`.
pub async fn tail_loop(redis: libs::redis_client::RedisHandle, base_dir: PathBuf) {
    let mut ticker = tokio::time::interval(TAIL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tracing::info!(
        "error-log tailer started: every {}s, {} services → redis '{}' (cap {})",
        TAIL_INTERVAL.as_secs(),
        SERVICES.len(),
        SystemKey::ERROR_LOGS,
        HISTORY_CAP,
    );

    loop {
        ticker.tick().await;

        // Load every service's cursor in one HGETALL.
        let cursors = redis.hgetall(SystemKey::ERROR_LOG_CURSORS).await;
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

        for service in SERVICES {
            let cursor = cursors
                .get(*service)
                .and_then(|v| Cursor::parse(v))
                // No cursor yet → start at the *end* of today's file so a fresh
                // deployment doesn't dump a full day of historical errors on the
                // first tick. (Reads `len()` below when offset would exceed it.)
                .unwrap_or(Cursor {
                    date: today.clone(),
                    offset: 0,
                });

            match tail_one(*service, &base_dir, &cursor, &today).await {
                Ok((entries, next)) => {
                    for entry in &entries {
                        match rmp_serde::to_vec_named(entry) {
                            Ok(blob) => {
                                redis
                                    .lpush_capped(SystemKey::ERROR_LOGS, &blob, HISTORY_CAP)
                                    .await
                            }
                            Err(e) => {
                                tracing::warn!("error-log tailer: encode failed: {e}")
                            }
                        }
                    }
                    // Persist the advanced cursor even when no new lines were
                    // read (e.g. a date rollover that reset offset to 0).
                    redis
                        .hset_multi(SystemKey::ERROR_LOG_CURSORS, &[(service, &next.encode())])
                        .await;
                }
                Err(e) => {
                    // Missing file (service never ran / not deployed here) is the
                    // common case on a partial host — log at debug, not warn.
                    tracing::debug!("error-log tailer: {service}: {e}");
                }
            }
        }
    }
}

/// Read one service's error log from `cursor` to EOF, returning the parsed new
/// entries and the advanced cursor. Handles daily rotation: if the cursor's
/// date isn't `today`, we read `today`'s file from offset 0 instead (the prior
/// day's file is final — anything in it was already consumed before midnight).
async fn tail_one(
    service: &str,
    base_dir: &Path,
    cursor: &Cursor,
    today: &str,
) -> std::io::Result<(Vec<LogEntry>, Cursor)> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    let path = base_dir
        .join(service)
        .join(format!("{today}.error.log"));

    // Date rollover (or first-ever read): start the new day's file at 0.
    let start = if cursor.date == today { cursor.offset } else { 0 };

    let mut file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        // No file today: keep a cursor pointing at today@0 so we pick the file
        // up the moment it's created, without re-reading yesterday.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Vec::new(), Cursor { date: today.to_string(), offset: 0 }));
        }
        Err(e) => return Err(e),
    };

    let len = file.metadata().await?.len();

    // Truncation guard: if the file is now shorter than our offset (rotated in
    // place, or replaced), restart from its current start.
    let start = if start > len { 0 } else { start };
    if start >= len {
        return Ok((Vec::new(), Cursor { date: today.to_string(), offset: len }));
    }

    let to_read = (len - start).min(MAX_READ_PER_PASS);
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut buf = vec![0u8; to_read as usize];
    file.read_exact(&mut buf).await?;

    // Only consume up to the last complete line so a half-written final line is
    // re-read (whole) next tick rather than split. `consumed` is what we
    // advance the cursor by.
    let text = String::from_utf8_lossy(&buf);
    let last_nl = text.rfind('\n');
    let (complete, consumed) = match last_nl {
        Some(idx) => (&text[..idx], idx + 1),
        // No newline in the chunk yet — nothing complete to emit; don't advance.
        None => return Ok((Vec::new(), Cursor { date: today.to_string(), offset: start })),
    };

    let entries: Vec<LogEntry> = complete
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| parse_line(service, l))
        .collect();

    Ok((
        entries,
        Cursor {
            date: today.to_string(),
            offset: start + consumed as u64,
        },
    ))
}

/// Best-effort parse of one tracing text line:
/// `2026-06-05T18:03:01.123456Z  WARN backend::routes: message…`
/// Falls back to `raw`-only when the shape doesn't match.
fn parse_line(service: &str, line: &str) -> LogEntry {
    let raw = line.to_string();
    let mut it = line.split_whitespace();

    // Timestamp: first token, must look like RFC3339 (contains 'T' and ends 'Z'
    // or has an offset). We don't fully validate — just gate on the 'T'.
    let ts = it
        .clone()
        .next()
        .filter(|t| t.contains('T') && t.len() >= 20)
        .map(|t| t.to_string());
    if ts.is_some() {
        it.next(); // consume the timestamp token
    }

    // Level: next token if it's one of the tracing levels.
    let level = it
        .clone()
        .next()
        .filter(|t| matches!(*t, "ERROR" | "WARN" | "INFO" | "DEBUG" | "TRACE"))
        .map(|t| t.to_string());
    if level.is_some() {
        it.next();
    }

    // Target: next token if it ends in ':' (tracing prints `target:`).
    let target = it
        .clone()
        .next()
        .filter(|t| t.ends_with(':'))
        .map(|t| t.trim_end_matches(':').to_string());

    LogEntry {
        service: service.to_string(),
        ts,
        level,
        target,
        raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips() {
        let c = Cursor { date: "2026-06-05".into(), offset: 4096 };
        let back = Cursor::parse(&c.encode()).expect("parse");
        assert_eq!(back.date, "2026-06-05");
        assert_eq!(back.offset, 4096);
        assert!(Cursor::parse("garbage").is_none());
        assert!(Cursor::parse("2026-06-05:notanumber").is_none());
    }

    #[test]
    fn parse_line_extracts_fields() {
        let l = "2026-06-05T18:03:01.123456Z  WARN backend::routes: redis GET failed";
        let e = parse_line("backend", l);
        assert_eq!(e.service, "backend");
        assert_eq!(e.ts.as_deref(), Some("2026-06-05T18:03:01.123456Z"));
        assert_eq!(e.level.as_deref(), Some("WARN"));
        assert_eq!(e.target.as_deref(), Some("backend::routes"));
        assert_eq!(e.raw, l);
    }

    #[test]
    fn parse_line_falls_back_to_raw() {
        let l = "  ...continuation of a multi-line backtrace frame";
        let e = parse_line("executor", l);
        assert_eq!(e.ts, None);
        assert_eq!(e.level, None);
        assert_eq!(e.raw, l);
    }

    #[tokio::test]
    async fn tail_one_reads_new_lines_and_advances() {
        use tokio::io::AsyncWriteExt;
        let day = "2026-06-05";
        let root = std::env::temp_dir().join(format!("logtail-test-{}", std::process::id()));
        let svc_dir = root.join("backend");
        tokio::fs::create_dir_all(&svc_dir).await.unwrap();
        let path = svc_dir.join(format!("{day}.error.log"));

        let line1 = "2026-06-05T18:00:00Z  WARN a::b: first\n";
        let line2 = "2026-06-05T18:00:01Z ERROR a::b: second\n";
        tokio::fs::write(&path, line1).await.unwrap();

        // From offset 0 we should read line1 and advance to its byte length.
        let c0 = Cursor { date: day.into(), offset: 0 };
        let (entries, c1) = tail_one("backend", &root, &c0, day).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level.as_deref(), Some("WARN"));
        assert_eq!(c1.offset, line1.len() as u64);

        // Append line2; from c1 we should read only line2.
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .unwrap();
        f.write_all(line2.as_bytes()).await.unwrap();
        let (entries2, c2) = tail_one("backend", &root, &c1, day).await.unwrap();
        assert_eq!(entries2.len(), 1);
        assert_eq!(entries2[0].level.as_deref(), Some("ERROR"));
        assert_eq!(c2.offset, (line1.len() + line2.len()) as u64);

        tokio::fs::remove_dir_all(&root).await.ok();
    }

    #[tokio::test]
    async fn tail_one_handles_partial_final_line() {
        let day = "2026-06-05";
        let root = std::env::temp_dir().join(format!("logtail-partial-{}", std::process::id()));
        let svc_dir = root.join("server");
        tokio::fs::create_dir_all(&svc_dir).await.unwrap();
        let path = svc_dir.join(format!("{day}.error.log"));

        // A complete line followed by a half-written one (no trailing newline).
        let content = "2026-06-05T18:00:00Z  WARN a::b: done\n2026-06-05T18:00:01Z ERROR a::b: par";
        tokio::fs::write(&path, content).await.unwrap();

        let c0 = Cursor { date: day.into(), offset: 0 };
        let (entries, c1) = tail_one("server", &root, &c0, day).await.unwrap();
        // Only the complete line is emitted; cursor stops at the newline so the
        // partial line is re-read whole next pass.
        assert_eq!(entries.len(), 1);
        assert_eq!(c1.offset, content.find('\n').unwrap() as u64 + 1);

        tokio::fs::remove_dir_all(&root).await.ok();
    }

    #[tokio::test]
    async fn tail_one_missing_file_keeps_today_cursor() {
        let root = std::env::temp_dir().join(format!("logtail-missing-{}", std::process::id()));
        let c = Cursor { date: "2026-06-04".into(), offset: 999 };
        let (entries, next) = tail_one("nope", &root, &c, "2026-06-05").await.unwrap();
        assert!(entries.is_empty());
        assert_eq!(next.date, "2026-06-05");
        assert_eq!(next.offset, 0);
    }
}
