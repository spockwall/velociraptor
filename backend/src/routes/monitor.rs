//! System monitor — host CPU / memory / disk + systemd unit status, plus a
//! persisted history for the frontend's line graph.
//!
//!   - `GET /api/monitor`         → live snapshot `{ host, cpu, memory, disks, services, ts }`
//!   - `GET /api/monitor/history` → recent samples (newest first) for charting
//!
//! ## Live reads
//! The frontend has no shell access to the box, so these handlers are the
//! bridge. Host metrics come from [`sysinfo`] (cross-platform). systemd unit
//! status is read by shelling out to `systemctl show` for each known
//! velociraptor unit — `ActiveState` / `SubState` / `MainPID` etc. On a host
//! without systemd (e.g. a dev mac) the `systemctl` call fails and every unit
//! comes back as `state: "unknown"` with an `error`, so the page still renders.
//!
//! ## History
//! [`sample_loop`] is a background task (spawned by `bin/backend.rs`) that
//! every [`SAMPLE_INTERVAL`] collects a [`MonitorStatus`] and:
//!   1. LPUSH-es it (msgpack) onto the capped Redis list [`System::METRICS`],
//!   2. appends it as one JSON line to `{logging.dir}/system/YYYY-MM-DD.log`.
//! `GET /api/monitor/history` reads the Redis list back. The disk log is the
//! durable record (Redis is capped + volatile).

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::response::Json;
use libs::redis_client::RedisKeyInfo;
use libs::redis_client::keys::System as SystemKey;
use serde::{Deserialize, Serialize};
use sysinfo::{Disks, System};

use crate::error::ApiError;
use crate::state::AppState;

/// How often the background sampler captures a snapshot.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);

/// Max samples retained in the Redis history list. 2880 × 30s ≈ 24h.
const HISTORY_CAP: usize = 2880;

/// Default number of samples returned by `GET /api/monitor/history`.
const HISTORY_DEFAULT_LIMIT: usize = 720;

/// systemd units that make up a velociraptor deployment. Mirrors
/// `deploy/systemd/velociraptor.target` (plus the mcp-monitor unit). Kept in
/// sync by hand — adding a unit there means adding it here.
const UNITS: &[&str] = &[
    "velociraptor-polymarket-recorder.service",
    "velociraptor-orderbook-recorder.service",
    "velociraptor-price-to-beat-fetcher.service",
    "velociraptor-asset-id-fetcher.service",
    "velociraptor-mcp-monitor.service",
];

#[derive(Serialize, Deserialize, Clone)]
pub struct MonitorStatus {
    /// Host identity + uptime so an operator can confirm which box this is.
    pub host: HostInfo,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub disks: Vec<DiskInfo>,
    /// Directory (du-style) usage of each immediate subfolder under the
    /// configured `data_dir`. Empty when the path doesn't exist (e.g. a dev
    /// box with no `/data`).
    pub data_usage: Vec<DataDirUsage>,
    pub services: Vec<ServiceInfo>,
    /// Server-side capture time (unix seconds), so the UI can show staleness.
    pub ts: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DataDirUsage {
    /// Immediate subfolder name under `data_dir` (e.g. `syslog`, `executor`).
    pub name: String,
    /// Recursive total size of the subfolder, in bytes (sum of file sizes).
    pub size_bytes: u64,
    /// This subfolder's share of the `data_dir` filesystem's total capacity,
    /// 0..100. 0.0 when the filesystem size is unknown.
    pub pct_of_fs: f32,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct HostInfo {
    pub hostname: String,
    pub os: String,
    pub kernel: String,
    /// Seconds since boot.
    pub uptime_secs: u64,
    pub cpu_count: usize,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CpuInfo {
    /// Aggregate CPU utilisation across all cores, 0..100.
    pub usage_pct: f32,
    /// Per-core utilisation, 0..100, in core order.
    pub per_core_pct: Vec<f32>,
    /// 1 / 5 / 15-minute load averages (0.0 on platforms without it, e.g.
    /// Windows; on Linux/mac these are the real `getloadavg` values).
    pub load_avg: [f64; 3],
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub used_pct: f32,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DiskInfo {
    pub mount_point: String,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub used_bytes: u64,
    pub used_pct: f32,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ServiceInfo {
    /// Full unit name, e.g. `velociraptor-orderbook-recorder.service`.
    pub unit: String,
    /// systemd `ActiveState`: active / inactive / failed / activating /
    /// deactivating, or `unknown` when `systemctl` couldn't be reached.
    pub active_state: String,
    /// systemd `SubState`: running / dead / exited / failed / auto-restart…
    pub sub_state: String,
    /// systemd `LoadState`: loaded / not-found / masked.
    pub load_state: String,
    /// Main process PID (0 when not running).
    pub main_pid: u32,
    /// Seconds the unit has been in its current active state (0 if unknown /
    /// not active). Derived from `ActiveEnterTimestamp`.
    pub active_secs: u64,
    /// Populated only when the unit could not be queried (no systemd, etc.).
    pub error: Option<String>,
}

/// Live-snapshot handler. Collects fresh host metrics + systemd status on
/// every call (no Redis read) so the page always shows the current instant.
pub(crate) async fn get_monitor(
    State(s): State<Arc<AppState>>,
) -> Result<Json<MonitorStatus>, ApiError> {
    let status = collect(s.data_dir.clone())
        .await
        .map_err(|e| ApiError::Decode(format!("monitor collect: {e}")))?;
    Ok(Json(status))
}

/// Live Redis key inventory: every key with its type and element count,
/// fetched fresh from Redis (cursor-based SCAN, non-blocking). Sorted by key.
/// `bba:` and `window_open_price:` entries are enriched with a decoded `data`
/// payload so the UI can show live prices instead of an opaque blob. Backs
/// `GET /api/redis/keys`.
pub(crate) async fn get_redis_keys(
    State(s): State<Arc<AppState>>,
) -> Result<Json<Vec<RedisKeyInfo>>, ApiError> {
    let mut keys = s.redis.key_overview().await;
    for k in &mut keys {
        if let Some(rest) = k.key.strip_prefix("bba:") {
            let _ = rest;
            if let Some(bytes) = s.redis.get_raw(&k.key).await {
                if let Ok(bba) = rmp_serde::from_slice::<libs::protocol::events::BbaPayload>(&bytes)
                {
                    k.data = serde_json::to_value(&bba).ok();
                }
            }
        } else if k.key.starts_with("window_open_price:") {
            // Stored as plain JSON text.
            if let Some(bytes) = s.redis.get_raw(&k.key).await {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    k.data = Some(v);
                }
            }
        }
    }
    Ok(Json(keys))
}

#[derive(Deserialize)]
pub(crate) struct HistoryQuery {
    /// How many samples to return (newest first). Clamped to `HISTORY_CAP`.
    pub limit: Option<usize>,
}

/// History handler — reads the capped Redis list written by [`sample_loop`].
/// Returns samples newest-first (the list order). The frontend reverses for
/// left-to-right time on the line graph.
pub(crate) async fn get_monitor_history(
    State(s): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<HistoryQuery>,
) -> Result<Json<Vec<MonitorStatus>>, ApiError> {
    let limit = q.limit.unwrap_or(HISTORY_DEFAULT_LIMIT).clamp(1, HISTORY_CAP);
    let raw = s.redis.lrange_raw(SystemKey::METRICS, 0, limit as isize - 1).await;

    let mut samples = Vec::with_capacity(raw.len());
    for blob in &raw {
        match rmp_serde::from_slice::<MonitorStatus>(blob) {
            Ok(m) => samples.push(m),
            // Skip a malformed entry rather than failing the whole read; the
            // history is best-effort and one bad blob shouldn't blank the graph.
            Err(e) => tracing::warn!("monitor history: skipping undecodable sample: {e}"),
        }
    }
    Ok(Json(samples))
}

/// Collect one full snapshot. `collect_host` and the `data_dir` directory walk
/// run on a blocking thread (CPU sampling sleeps; the walk hits the filesystem);
/// `collect_services` shells out concurrently.
///
/// `data_dir` is the root whose immediate subfolders we report du-style usage
/// for (see [`collect_data_usage`]).
pub async fn collect(data_dir: std::path::PathBuf) -> Result<MonitorStatus, String> {
    let (host, cpu, memory, disks) = tokio::task::spawn_blocking(collect_host)
        .await
        .map_err(|e| format!("host collect join: {e}"))?;
    let data_usage = tokio::task::spawn_blocking(move || collect_data_usage(&data_dir))
        .await
        .map_err(|e| format!("data usage join: {e}"))?;
    let services = collect_services().await;
    Ok(MonitorStatus {
        host,
        cpu,
        memory,
        disks,
        data_usage,
        services,
        ts: chrono::Utc::now().timestamp(),
    })
}

/// Background sampler: every [`SAMPLE_INTERVAL`], capture a snapshot, push it
/// onto the capped Redis history list, and append it to today's disk log.
/// Spawned once by `bin/backend.rs`; loops until the process exits.
///
/// `syslog_dir` is the configured logging directory (`cfg.logging.dir`); the
/// per-day files live under `{syslog_dir}/system/`. `data_dir` is the root
/// whose subfolder usage each sample records (`backend.data_dir`).
pub async fn sample_loop(
    redis: libs::redis_client::RedisHandle,
    syslog_dir: std::path::PathBuf,
    data_dir: std::path::PathBuf,
) {
    let log_dir = syslog_dir.join("system");
    if let Err(e) = tokio::fs::create_dir_all(&log_dir).await {
        tracing::error!("monitor sampler: cannot create {}: {e}", log_dir.display());
        // Keep going — Redis history still works even if disk logging can't.
    }

    let mut ticker = tokio::time::interval(SAMPLE_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tracing::info!(
        "monitor sampler started: every {}s → redis '{}' (cap {}) + {}/YYYY-MM-DD.log",
        SAMPLE_INTERVAL.as_secs(),
        SystemKey::METRICS,
        HISTORY_CAP,
        log_dir.display(),
    );

    loop {
        ticker.tick().await;

        let status = match collect(data_dir.clone()).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("monitor sampler: collect failed: {e}");
                continue;
            }
        };

        // Redis history (msgpack, capped, newest-first).
        match rmp_serde::to_vec_named(&status) {
            Ok(blob) => {
                redis
                    .lpush_capped(SystemKey::METRICS, &blob, HISTORY_CAP)
                    .await
            }
            Err(e) => tracing::warn!("monitor sampler: encode failed: {e}"),
        }

        // Disk log (one JSON line per sample, daily file).
        if let Err(e) = append_disk_log(&log_dir, &status).await {
            tracing::warn!("monitor sampler: disk log write failed: {e}");
        }
    }
}

/// Append `status` as a single JSON line to `{log_dir}/YYYY-MM-DD.log`. The
/// date is derived from the sample's `ts` so a sample taken just after
/// midnight still lands in the correct day's file.
async fn append_disk_log(
    log_dir: &std::path::Path,
    status: &MonitorStatus,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    let day = chrono::DateTime::from_timestamp(status.ts, 0)
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y-%m-%d");
    let path = log_dir.join(format!("{day}.log"));

    let mut line = serde_json::to_string(status)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push('\n');

    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await?;
    f.write_all(line.as_bytes()).await
}

/// Blocking host-metric collection (CPU/mem/disk). Runs on a blocking thread.
fn collect_host() -> (HostInfo, CpuInfo, MemoryInfo, Vec<DiskInfo>) {
    let mut sys = System::new();

    // CPU usage needs two samples MINIMUM_CPU_UPDATE_INTERVAL apart.
    sys.refresh_cpu_all();
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(200)));
    sys.refresh_cpu_all();
    sys.refresh_memory();

    let per_core_pct: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
    let usage_pct = if per_core_pct.is_empty() {
        0.0
    } else {
        per_core_pct.iter().sum::<f32>() / per_core_pct.len() as f32
    };
    let load = System::load_average();

    let total = sys.total_memory();
    let used = sys.used_memory();
    let available = sys.available_memory();
    let used_pct = if total > 0 {
        used as f32 / total as f32 * 100.0
    } else {
        0.0
    };

    let host = HostInfo {
        hostname: System::host_name().unwrap_or_else(|| "unknown".into()),
        os: System::long_os_version().unwrap_or_else(|| "unknown".into()),
        kernel: System::kernel_version().unwrap_or_else(|| "unknown".into()),
        uptime_secs: System::uptime(),
        cpu_count: per_core_pct.len(),
    };

    let cpu = CpuInfo {
        usage_pct,
        per_core_pct,
        load_avg: [load.one, load.five, load.fifteen],
    };

    let memory = MemoryInfo {
        total_bytes: total,
        used_bytes: used,
        available_bytes: available,
        used_pct,
        swap_total_bytes: sys.total_swap(),
        swap_used_bytes: sys.used_swap(),
    };

    let disks = collect_disks(&Disks::new_with_refreshed_list());

    (host, cpu, memory, disks)
}

/// One raw `(mount, total, available)` tuple — the slice of a sysinfo `Disk`
/// that [`dedup_disks`] needs. Pulled out so the filter logic is unit testable
/// without constructing real `Disk`s (which can't be built by hand).
struct RawDisk {
    mount: String,
    total: u64,
    available: u64,
}

/// Mount-path prefixes that are never real disks the operator cares about. In a
/// container `sysinfo` reports every visible mount, so Docker's injected file
/// mounts (`/etc/hosts`, `/etc/hostname`, `/etc/resolv.conf`) and our own app
/// bind mounts (`/app/...`) show up — the bind from the host even reports a
/// bogus multi-TB size. Anything under these prefixes is dropped.
const NOISE_MOUNT_PREFIXES: &[&str] = &["/etc", "/app", "/proc", "/sys", "/dev", "/run", "/var/lib/docker"];

/// Turn the refreshed `Disks` list into the cleaned `DiskInfo` we surface.
fn collect_disks(disks: &Disks) -> Vec<DiskInfo> {
    let raw: Vec<RawDisk> = disks
        .iter()
        .map(|d| RawDisk {
            mount: d.mount_point().to_string_lossy().into_owned(),
            total: d.total_space(),
            available: d.available_space(),
        })
        .collect();
    dedup_disks(raw)
}

/// Whether a mount path is under one of the noise prefixes. Matches the prefix
/// itself or a path segment below it (so `/etc` and `/etc/hosts` match, but a
/// hypothetical `/etcdata` does not).
fn is_noise_mount(mount: &str) -> bool {
    NOISE_MOUNT_PREFIXES
        .iter()
        .any(|p| mount == *p || mount.starts_with(&format!("{p}/")))
}

/// Filter the raw mount list down to the real filesystems we show.
///
/// Two passes:
/// 1. **Drop noise mounts** ([`NOISE_MOUNT_PREFIXES`]) and zero-total pseudo
///    filesystems. This removes the bogus multi-TB `/app/...` bind and the
///    `/etc/...` file mounts.
/// 2. **De-dup by `(total, available)`**. Docker can surface the same backing
///    filesystem under several mounts with *different* synthetic device names
///    (e.g. `/` and `/etc/hosts` report byte-identical stats), so keying on the
///    reported size collapses them; the shortest mount path wins (`/` over a
///    nested mount). Genuinely separate volumes differ in size and are kept.
fn dedup_disks(raw: Vec<RawDisk>) -> Vec<DiskInfo> {
    use std::collections::HashMap;

    // (total, available) → chosen entry (shortest mount path wins).
    let mut by_size: HashMap<(u64, u64), RawDisk> = HashMap::new();
    for d in raw {
        if d.total == 0 || is_noise_mount(&d.mount) {
            continue;
        }
        let key = (d.total, d.available);
        match by_size.get(&key) {
            Some(existing) if existing.mount.len() <= d.mount.len() => {}
            _ => {
                by_size.insert(key, d);
            }
        }
    }

    let mut out: Vec<DiskInfo> = by_size
        .into_values()
        .map(|d| {
            let used = d.total.saturating_sub(d.available);
            DiskInfo {
                mount_point: d.mount,
                total_bytes: d.total,
                available_bytes: d.available,
                used_bytes: used,
                used_pct: if d.total > 0 {
                    used as f32 / d.total as f32 * 100.0
                } else {
                    0.0
                },
            }
        })
        .collect();

    // Stable, readable order: by mount point.
    out.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    out
}

/// du-style usage of each immediate subfolder under `data_dir`.
///
/// For each direct child *directory* of `data_dir`, recursively sums file
/// sizes (`dir_size`) and computes its share of the `data_dir` filesystem's
/// total capacity. Files directly under `data_dir` (not in a subfolder) are
/// ignored — we report folders. Result is sorted largest-first.
///
/// A missing `data_dir` (the common dev case — no `/data` on a mac) yields an
/// empty vec, not an error. Blocking I/O: call from a blocking thread.
fn collect_data_usage(data_dir: &std::path::Path) -> Vec<DataDirUsage> {
    let entries = match std::fs::read_dir(data_dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(), // absent / unreadable → empty, no error
    };

    // Filesystem total capacity for the percentage. Best-effort; 0 → pct 0.
    let fs_total = Disks::new_with_refreshed_list()
        .iter()
        .filter(|d| data_dir.starts_with(d.mount_point()))
        // Most specific (longest) mount point that contains `data_dir`.
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.total_space())
        .unwrap_or(0);

    let mut out: Vec<DataDirUsage> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| {
            let size_bytes = dir_size(&e.path());
            DataDirUsage {
                name: e.file_name().to_string_lossy().into_owned(),
                size_bytes,
                pct_of_fs: if fs_total > 0 {
                    (size_bytes as f64 / fs_total as f64 * 100.0) as f32
                } else {
                    0.0
                },
            }
        })
        .collect();

    out.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    out
}

/// Recursively sum the byte size of every regular file under `path`. Symlinks
/// are not followed (so we don't double-count or escape the tree); unreadable
/// entries are skipped. Iterative (explicit stack) to avoid deep-recursion
/// blowups on pathological trees.
fn dir_size(path: &std::path::Path) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            // lstat (don't follow symlinks): `DirEntry::metadata` already does
            // NOT traverse symlinks on Unix, but be explicit for clarity.
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            let ft = meta.file_type();
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                total += meta.len();
            }
            // symlinks / sockets / etc. contribute nothing.
        }
    }
    total
}

/// Query every velociraptor unit via `systemctl show`. One process per unit.
/// The unit count is tiny and fixed, so a sequential await keeps the code
/// simple and avoids pulling in `futures` for `join_all`. A failure (no
/// systemd, unit not found) yields a `ServiceInfo` with
/// `active_state: "unknown"` and a populated `error`.
async fn collect_services() -> Vec<ServiceInfo> {
    let mut out = Vec::with_capacity(UNITS.len());
    for unit in UNITS {
        out.push(query_unit(unit).await);
    }
    out
}

async fn query_unit(unit: &str) -> ServiceInfo {
    // `systemctl show` prints `Key=Value` lines and exits 0 even for an
    // unknown unit (it reports LoadState=not-found). We request only the
    // fields we surface to keep the output small.
    let output = tokio::process::Command::new("systemctl")
        .args([
            "show",
            unit,
            "--no-pager",
            "--property=ActiveState,SubState,LoadState,MainPID,ActiveEnterTimestampMonotonic",
        ])
        .output()
        .await;

    let out = match output {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            return unknown_service(unit, format!("systemctl exited non-zero: {err}"));
        }
        Err(e) => return unknown_service(unit, format!("systemctl unavailable: {e}")),
    };

    let text = String::from_utf8_lossy(&out.stdout);
    let mut active_state = String::new();
    let mut sub_state = String::new();
    let mut load_state = String::new();
    let mut main_pid: u32 = 0;
    let mut active_enter_monotonic: u64 = 0;

    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k {
            "ActiveState" => active_state = v.to_string(),
            "SubState" => sub_state = v.to_string(),
            "LoadState" => load_state = v.to_string(),
            "MainPID" => main_pid = v.parse().unwrap_or(0),
            "ActiveEnterTimestampMonotonic" => {
                active_enter_monotonic = v.parse().unwrap_or(0)
            }
            _ => {}
        }
    }

    // `ActiveEnterTimestampMonotonic` is microseconds since boot. Convert to
    // "seconds active" using the host uptime (also monotonic-from-boot).
    let active_secs = if active_state == "active" && active_enter_monotonic > 0 {
        let uptime_us = System::uptime().saturating_mul(1_000_000);
        uptime_us
            .saturating_sub(active_enter_monotonic)
            .saturating_div(1_000_000)
    } else {
        0
    };

    ServiceInfo {
        unit: unit.to_string(),
        active_state,
        sub_state,
        load_state,
        main_pid,
        active_secs,
        error: None,
    }
}

fn unknown_service(unit: &str, error: String) -> ServiceInfo {
    ServiceInfo {
        unit: unit.to_string(),
        active_state: "unknown".to_string(),
        sub_state: String::new(),
        load_state: String::new(),
        main_pid: 0,
        active_secs: 0,
        error: Some(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_disks_collapses_container_mount_noise() {
        // Exact bytes observed from the in-container `/api/monitor` in the bug
        // report: `/` and `/etc/hosts` are byte-identical (same backing fs,
        // different synthetic device), and `/app/syslog` is a bind reporting a
        // bogus ~57 TB. The real root is ~240 GB.
        let root_total = 240_120_725_504u64;
        let root_avail = 181_098_192_896u64;
        let bogus_total = 62_747_442_151_424u64; // ~57 TiB
        let bogus_avail = 2_182_847_922_176u64;

        let raw = vec![
            RawDisk { mount: "/".into(), total: root_total, available: root_avail },
            RawDisk { mount: "/app/syslog".into(), total: bogus_total, available: bogus_avail },
            RawDisk { mount: "/app/configs".into(), total: bogus_total, available: bogus_avail },
            RawDisk { mount: "/etc/hosts".into(), total: root_total, available: root_avail },
            RawDisk { mount: "/etc/hostname".into(), total: root_total, available: root_avail },
            RawDisk { mount: "/etc/resolv.conf".into(), total: root_total, available: root_avail },
            // A genuinely separate data volume — must be kept.
            RawDisk { mount: "/data".into(), total: 500_000_000_000, available: 100_000_000_000 },
            // A pseudo filesystem with zero total — must be dropped.
            RawDisk { mount: "/proc/sys".into(), total: 0, available: 0 },
        ];

        let out = dedup_disks(raw);

        // Only the two real filesystems survive; every /app and /etc mount (incl.
        // the bogus 57 TB) is gone.
        let mounts: Vec<&str> = out.iter().map(|d| d.mount_point.as_str()).collect();
        assert_eq!(mounts, vec!["/", "/data"], "got: {mounts:?}");

        // No surviving entry reports the bogus multi-TB size.
        assert!(out.iter().all(|d| d.total_bytes < 1_000_000_000_000), "a TB-scale phantom disk leaked through");

        let root = &out[0];
        assert_eq!(root.total_bytes, root_total);
        assert!((root.used_pct - 24.6).abs() < 0.5, "used_pct ~24.6, got {}", root.used_pct);
    }

    #[test]
    fn data_usage_sums_subfolders_and_sorts_largest_first() {
        let root = std::env::temp_dir().join(format!("datausage-{}-{}", std::process::id(), line!()));
        // small/a.txt = 10 bytes; big/nested/b.bin = 1000 bytes; a loose file
        // directly under root (ignored — we report folders only).
        std::fs::create_dir_all(root.join("small")).unwrap();
        std::fs::create_dir_all(root.join("big/nested")).unwrap();
        std::fs::write(root.join("small/a.txt"), vec![0u8; 10]).unwrap();
        std::fs::write(root.join("big/nested/b.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(root.join("loose.txt"), vec![0u8; 5]).unwrap();

        let usage = collect_data_usage(&root);

        assert_eq!(usage.len(), 2, "two subfolders, loose file ignored");
        // Sorted largest-first.
        assert_eq!(usage[0].name, "big");
        assert_eq!(usage[0].size_bytes, 1000);
        assert_eq!(usage[1].name, "small");
        assert_eq!(usage[1].size_bytes, 10);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn data_usage_missing_dir_is_empty_not_error() {
        let usage = collect_data_usage(std::path::Path::new("/no/such/data/dir/here"));
        assert!(usage.is_empty());
    }

    #[test]
    fn is_noise_mount_matches_prefix_segments_only() {
        assert!(is_noise_mount("/etc"));
        assert!(is_noise_mount("/etc/hosts"));
        assert!(is_noise_mount("/app/syslog"));
        assert!(is_noise_mount("/proc/sys"));
        // Real mounts are kept.
        assert!(!is_noise_mount("/"));
        assert!(!is_noise_mount("/data"));
        // A name that merely starts with a noise prefix but isn't a child path.
        assert!(!is_noise_mount("/etcdata"));
        assert!(!is_noise_mount("/application"));
    }

    #[test]
    fn host_collection_returns_sane_values() {
        let (host, cpu, memory, _disks) = collect_host();
        assert!(host.cpu_count >= 1, "expected at least one core");
        assert_eq!(cpu.per_core_pct.len(), host.cpu_count);
        assert!(cpu.usage_pct >= 0.0 && cpu.usage_pct <= 100.0);
        assert!(memory.total_bytes > 0, "total memory should be non-zero");
        assert!(memory.used_bytes <= memory.total_bytes);
    }

    #[tokio::test]
    async fn services_query_covers_every_unit() {
        // On a host without these units (CI / dev mac) each entry comes back
        // either as the real systemd state or as `unknown` with an error —
        // never a panic, and always one entry per configured unit.
        let services = collect_services().await;
        assert_eq!(services.len(), UNITS.len());
        for s in &services {
            assert!(!s.unit.is_empty());
            if s.active_state == "unknown" {
                assert!(s.error.is_some());
            }
        }
    }

    #[tokio::test]
    async fn collect_msgpack_round_trips() {
        // The history path encodes with rmp_serde::to_vec_named and decodes
        // with rmp_serde::from_slice — exercise that round-trip end to end.
        let status = collect(std::path::PathBuf::from("/nonexistent-data-dir"))
            .await
            .expect("collect");
        let blob = rmp_serde::to_vec_named(&status).expect("encode");
        let back: MonitorStatus = rmp_serde::from_slice(&blob).expect("decode");
        assert_eq!(back.ts, status.ts);
        assert_eq!(back.host.cpu_count, status.host.cpu_count);
        assert_eq!(back.services.len(), status.services.len());
        assert_eq!(back.data_usage.len(), status.data_usage.len());
    }

    #[tokio::test]
    async fn disk_log_appends_json_lines_to_daily_file() {
        let dir = std::env::temp_dir().join(format!("mon-test-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let status = collect(std::path::PathBuf::from("/nonexistent-data-dir"))
            .await
            .expect("collect");
        append_disk_log(&dir, &status).await.expect("write 1");
        append_disk_log(&dir, &status).await.expect("write 2");

        let day = chrono::DateTime::from_timestamp(status.ts, 0)
            .unwrap()
            .format("%Y-%m-%d");
        let path = dir.join(format!("{day}.log"));
        let body = tokio::fs::read_to_string(&path).await.expect("read back");

        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2, "append should produce one line per call");
        // Each line is a self-contained JSON object that decodes back.
        let parsed: MonitorStatus = serde_json::from_str(lines[0]).expect("valid json line");
        assert_eq!(parsed.ts, status.ts);

        tokio::fs::remove_dir_all(&dir).await.ok();
    }
}
