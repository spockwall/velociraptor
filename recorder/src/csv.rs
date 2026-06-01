//! Shared CSV archive helper.
//!
//! One small, reusable layer for the project's append-only CSV datasets:
//! the recorder's user-event stream, the price-to-beat fetcher, the asset-id
//! fetcher, and any future low-volume tabular data. It owns the three
//! operations every such dataset needs and were previously copy-pasted across
//! call sites:
//!
//!   - **append a row**, writing the header only when the file is newly created
//!     (so a restart appends instead of re-heading),
//!   - **read every row** in a file back into a typed struct,
//!   - **build a daily file path** from path components + a date.
//!
//! Rows are any `serde` `Serialize` (write) / `DeserializeOwned` (read) struct
//! with flat fields — the `csv` crate maps field names ↔ columns. Files are
//! never rotated or compressed here; CSV is for human-readable, low-volume data
//! (high-volume orderbook/trade streams go to mpack via [`crate::StorageWriter`]).
//!
//! # Example
//! ```no_run
//! use recorder::CsvArchive;
//! use serde::{Serialize, Deserialize};
//! # use chrono::Utc;
//!
//! #[derive(Serialize, Deserialize)]
//! struct Row { ts: i64, slug: String, value: f64 }
//!
//! let archive = CsvArchive::new("data/my_dataset");
//! let path = archive.daily_path(&["polymarket", "btc-updown-15m"], Utc::now());
//! archive.append(&path, &Row { ts: 1, slug: "x".into(), value: 0.5 }).unwrap();
//!
//! // Read every row already written for this market (across all days):
//! let rows: Vec<Row> = archive.read_dir(&["polymarket", "btc-updown-15m"]);
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// A rooted, append-only CSV store. Cheap to clone (just a `PathBuf`).
#[derive(Debug, Clone)]
pub struct CsvArchive {
    root: PathBuf,
}

impl CsvArchive {
    /// Create an archive rooted at `root`. Directories are created lazily on
    /// the first write to a given path.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The archive root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Build a daily file path: `{root}/{parts...}/{YYYY-MM-DD}.csv`.
    ///
    /// `parts` are the partition components (e.g. `["polymarket", base_slug]`).
    /// The date is formatted UTC; callers that key by *window start* should
    /// pass the window-start date, not write time.
    pub fn daily_path(&self, parts: &[&str], day: DateTime<Utc>) -> PathBuf {
        let mut p = self.root.clone();
        for part in parts {
            p.push(part);
        }
        p.push(format!("{}.csv", day.format("%Y-%m-%d")));
        p
    }

    /// The partition directory `{root}/{parts...}/` (without a filename).
    pub fn partition_dir(&self, parts: &[&str]) -> PathBuf {
        let mut p = self.root.clone();
        for part in parts {
            p.push(part);
        }
        p
    }

    /// Append `row` to the CSV at `path`, creating parent dirs as needed and
    /// writing the header row only when the file is new (or empty). Flushes
    /// before returning so the row is durable for the next reader.
    pub fn append<T: Serialize>(&self, path: &Path, row: &T) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("mkdir {}", parent.display()))?;
        }
        // Header only when the file doesn't yet exist or is empty — this keeps
        // append-on-restart from injecting a second header mid-file.
        let needs_header = !path.exists() || path.metadata().map(|m| m.len() == 0).unwrap_or(true);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open {}", path.display()))?;
        let mut wtr = csv::WriterBuilder::new()
            .has_headers(needs_header)
            .from_writer(file);
        wtr.serialize(row).context("serialize row")?;
        wtr.flush().context("flush csv")?;
        Ok(())
    }

    /// Read every row from a single CSV file. Missing file → empty vec;
    /// malformed rows are skipped (best-effort, like the original fetchers).
    pub fn read_file<T: DeserializeOwned>(&self, path: &Path) -> Vec<T> {
        let Ok(mut rdr) = csv::Reader::from_path(path) else {
            return Vec::new();
        };
        rdr.deserialize::<T>().flatten().collect()
    }

    /// Read every row across every `*.csv` file in a partition directory
    /// `{root}/{parts...}/`. Used to rebuild a dedup set on startup. Missing
    /// directory → empty vec.
    pub fn read_dir<T: DeserializeOwned>(&self, parts: &[&str]) -> Vec<T> {
        let dir = self.partition_dir(parts);
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|s| s.to_str()) != Some("csv") {
                continue;
            }
            out.extend(self.read_file::<T>(&p));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Row {
        ts: i64,
        slug: String,
        value: f64,
    }

    fn tmp() -> PathBuf {
        // Per-call unique dir: a process-wide atomic counter avoids the
        // clock-resolution collisions that a timestamp-based name hits when
        // tests run in parallel.
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "csvarchive-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn append_writes_header_once_then_appends() {
        let root = tmp();
        let archive = CsvArchive::new(&root);
        let day = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let path = archive.daily_path(&["polymarket", "btc-updown-15m"], day);

        archive.append(&path, &Row { ts: 1, slug: "a".into(), value: 0.5 }).unwrap();
        archive.append(&path, &Row { ts: 2, slug: "b".into(), value: 0.6 }).unwrap();

        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 3, "header + 2 rows");
        assert_eq!(lines[0], "ts,slug,value");
        assert!(lines[1].starts_with("1,a,"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_dir_collects_rows_across_files() {
        let root = tmp();
        let archive = CsvArchive::new(&root);
        let d1 = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let d2 = DateTime::from_timestamp(1_700_100_000, 0).unwrap();
        archive.append(&archive.daily_path(&["ex", "m"], d1), &Row { ts: 1, slug: "a".into(), value: 0.1 }).unwrap();
        archive.append(&archive.daily_path(&["ex", "m"], d2), &Row { ts: 2, slug: "b".into(), value: 0.2 }).unwrap();

        let mut rows: Vec<Row> = archive.read_dir(&["ex", "m"]);
        rows.sort_by_key(|r| r.ts);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], Row { ts: 1, slug: "a".into(), value: 0.1 });

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_missing_is_empty() {
        let archive = CsvArchive::new(tmp());
        let rows: Vec<Row> = archive.read_dir(&["nope"]);
        assert!(rows.is_empty());
    }
}
