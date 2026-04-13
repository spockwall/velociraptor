//! Spillover writer for event records evicted from capped Redis lists.
//!
//! When `LPUSH` + `LTRIM` drops an entry off the end of `events:*`, that
//! entry is handed to a `SpilloverWriter` which appends it to a daily
//! `.mpack.zst` file on disk. See `docs/redis.md`.
//!
//! Full implementation arrives in Phase 5. This file defines the public
//! type surface only.

use anyhow::Result;
use std::path::PathBuf;

/// Writes evicted event records to daily `.mpack.zst` files.
///
/// File layout: `{base_path}/{kind}/{YYYY-MM-DD}.mpack.zst` where `kind`
/// is the stringified `EventKind` (e.g. `fills`, `orders`, `log`).
pub struct SpilloverWriter {
    _base_path: PathBuf,
}

impl SpilloverWriter {
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self { _base_path: base_path.into() }
    }

    /// Append an evicted event record. Phase 5 will implement the actual
    /// write path; today this is a no-op so the API can be wired up.
    pub async fn append(&self, _kind: &str, _payload: &[u8]) -> Result<()> {
        Ok(())
    }
}
