//! Compact connection / data-flow status bar.
//!
//! Rendered as a small block of plain lines that can be prepended to an
//! existing inline UI (e.g. [`super::polymarket::PolymarketUi`]) via
//! `set_status_lines`. It owns no cursor state of its own so it composes
//! safely with any parent renderer.
//!
//! # Rows
//!
//! One row per tracked series (e.g. `KXBTC15M`, `KXETH15M`). Each row shows:
//! - **CONN**   : `UP` / `DOWN` / `…` (connecting)
//! - **DATA**   : `LIVE` (snapshot within `stale_after`) / `STALE` / `NONE`
//! - **AGE**    : seconds since last snapshot
//! - **NEXT**   : `PRE` (next window pre-started) / `—` (idle) + ticker suffix
//! - **WIN**    : seconds remaining in the current window
//!
//! Colors match the rest of the UI: green = good, yellow = warn, red = bad.
//!
//! # Example
//!
//! ```text
//! ┌ status ─────────────────────────────────────────────────────────────┐
//! │ KXBTC15M  conn=UP   data=LIVE  age=   0s  next=PRE 26APR161315  win= 42s │
//! │ KXETH15M  conn=UP   data=LIVE  age=   1s  next=—                win= 42s │
//! └────────────────────────────────────────────────────────────────────┘
//! ```

use super::ansi::*;
use super::panel::hline;
use std::collections::HashMap;
use std::time::{Duration, Instant};

// ── Public data types ─────────────────────────────────────────────────────────

/// WebSocket connection state for one series.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnState {
    /// Connection hasn't been attempted yet.
    Idle,
    /// Handshake in progress.
    Connecting,
    /// Subscribed and receiving frames.
    Up,
    /// Disconnected or errored.
    Down,
}

impl ConnState {
    fn label(self) -> &'static str {
        match self {
            ConnState::Idle => "IDLE",
            ConnState::Connecting => "…",
            ConnState::Up => "UP",
            ConnState::Down => "DOWN",
        }
    }
    fn color(self) -> &'static str {
        match self {
            ConnState::Idle => DIM,
            ConnState::Connecting => CYAN,
            ConnState::Up => GREEN,
            ConnState::Down => RED,
        }
    }
}

/// Whether the next-window connection has been pre-started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NextState {
    /// Current window only — no successor running yet.
    Idle,
    /// Next window has been pre-started and is running in parallel.
    PreStarted,
}

/// State for one tracked series.
#[derive(Clone, Debug)]
pub struct SeriesStatus {
    /// Stable panel key, e.g. `"KXBTC15M"`.
    pub series: String,
    /// Full current-window market ticker.
    pub current_ticker: String,
    /// Full next-window market ticker (populated once pre-started).
    pub next_ticker: Option<String>,
    /// WS connection state for the current window.
    pub conn: ConnState,
    /// Last time a snapshot arrived (any type — full or delta).
    pub last_update: Option<Instant>,
    /// Rotation sub-state.
    pub next: NextState,
    /// UTC seconds remaining until the current window closes.
    pub window_secs_remaining: u64,
}

impl SeriesStatus {
    pub fn new(series: impl Into<String>) -> Self {
        Self {
            series: series.into(),
            current_ticker: String::new(),
            next_ticker: None,
            conn: ConnState::Idle,
            last_update: None,
            next: NextState::Idle,
            window_secs_remaining: 0,
        }
    }
}

// ── StatusBar ─────────────────────────────────────────────────────────────────

/// Compact per-series status panel. Produces a line buffer; caller is
/// responsible for printing (or composing into another UI).
pub struct StatusBar {
    order: Vec<String>,
    rows: HashMap<String, SeriesStatus>,
    /// Snapshot age above this marks data as STALE.
    stale_after: Duration,
    /// Target panel width in display columns.
    width: usize,
}

impl StatusBar {
    /// Create a new status bar. `width` is clamped to `[60, 140]`.
    pub fn new(width: usize, stale_after: Duration) -> Self {
        Self {
            order: Vec::new(),
            rows: HashMap::new(),
            stale_after,
            width: width.clamp(60, 140),
        }
    }

    /// Register a series (idempotent). Creates the row in its initial state.
    pub fn track(&mut self, series: &str) {
        if !self.rows.contains_key(series) {
            self.order.push(series.to_string());
            self.rows
                .insert(series.to_string(), SeriesStatus::new(series));
        }
    }

    /// Update the current-window ticker and bump connection state.
    pub fn set_current(&mut self, series: &str, ticker: &str, conn: ConnState) {
        self.track(series);
        let row = self.rows.get_mut(series).expect("tracked");
        row.current_ticker = ticker.to_string();
        row.conn = conn;
    }

    /// Update just the connection state (e.g. on WS disconnect / reconnect).
    pub fn set_conn(&mut self, series: &str, conn: ConnState) {
        self.track(series);
        self.rows.get_mut(series).expect("tracked").conn = conn;
    }

    /// Record that a snapshot or delta just arrived for this series.
    pub fn mark_data(&mut self, series: &str) {
        self.track(series);
        self.rows.get_mut(series).expect("tracked").last_update = Some(Instant::now());
    }

    /// Indicate the next window has been pre-started (or cleared on swap).
    pub fn set_next(&mut self, series: &str, ticker: Option<&str>) {
        self.track(series);
        let row = self.rows.get_mut(series).expect("tracked");
        row.next_ticker = ticker.map(|t| t.to_string());
        row.next = if ticker.is_some() {
            NextState::PreStarted
        } else {
            NextState::Idle
        };
    }

    /// Set remaining seconds until the current window closes.
    pub fn set_window_remaining(&mut self, series: &str, secs: u64) {
        self.track(series);
        self.rows
            .get_mut(series)
            .expect("tracked")
            .window_secs_remaining = secs;
    }

    /// Total number of lines this bar will emit (title + rows + bottom).
    pub fn line_count(&self) -> usize {
        self.rows.len() + 2
    }

    /// Configured width.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Render the status bar as a list of ANSI-formatted lines.
    pub fn render(&self) -> Vec<String> {
        let inner = self.width.saturating_sub(2);
        let now = Instant::now();
        let mut lines = Vec::with_capacity(self.rows.len() + 2);

        // Title
        let title_plain = " status ";
        let title_styled = format!(" {BOLD}{CYAN}status{RESET} ");
        let pad = inner.saturating_sub(title_plain.chars().count());
        lines.push(format!("┌{}{}┐", title_styled, hline(pad)));

        // Rows
        for key in &self.order {
            let Some(row) = self.rows.get(key) else {
                continue;
            };
            lines.push(render_row(row, inner, now, self.stale_after));
        }

        // Bottom
        lines.push(format!("└{}┘", hline(inner)));
        lines
    }
}

// ── Row rendering ─────────────────────────────────────────────────────────────

fn render_row(row: &SeriesStatus, inner: usize, now: Instant, stale_after: Duration) -> String {
    let (data_label, data_color, age_secs) = match row.last_update {
        None => ("NONE", RED, 0u64),
        Some(t) => {
            let age = now.saturating_duration_since(t);
            if age <= stale_after {
                ("LIVE", GREEN, age.as_secs())
            } else {
                ("STALE", RED, age.as_secs())
            }
        }
    };

    let (next_label, next_color) = match row.next {
        NextState::Idle => ("—".to_string(), DIM),
        NextState::PreStarted => {
            let suffix = row
                .next_ticker
                .as_deref()
                .and_then(trailing_suffix)
                .unwrap_or("?");
            (format!("PRE {}", suffix), GREEN)
        }
    };

    let win_color = if row.window_secs_remaining <= 10 {
        RED
    } else if row.window_secs_remaining <= 30 {
        CYAN
    } else {
        DIM
    };

    // Plain (for width calc) + styled versions.
    let plain = format!(
        " {series:<10}  conn={conn_l:<6}  data={data_l:<5}  age={age:>4}s  next={next_l:<16}  win={win:>4}s ",
        series = row.series,
        conn_l = row.conn.label(),
        data_l = data_label,
        age = age_secs,
        next_l = next_label,
        win = row.window_secs_remaining,
    );
    let styled = format!(
        " {BOLD}{cyan}{series:<10}{RESET}  conn={cc}{conn_l:<6}{RESET}  data={dc}{data_l:<5}{RESET}  age={age:>4}s  next={nc}{next_l:<16}{RESET}  win={wc}{win:>4}s{RESET} ",
        series = row.series,
        cyan = CYAN,
        cc = row.conn.color(),
        conn_l = row.conn.label(),
        dc = data_color,
        data_l = data_label,
        age = age_secs,
        nc = next_color,
        next_l = next_label,
        wc = win_color,
        win = row.window_secs_remaining,
    );

    let pad = inner.saturating_sub(plain.chars().count());
    format!("│{}{}│", styled, " ".repeat(pad))
}

/// Return the last dash-separated segment of a ticker, e.g.
/// `"KXBTC15M-26APR161315-00"` → `"26APR161315-00"` when called with one
/// level (we want the close-time slot for readability).
fn trailing_suffix(ticker: &str) -> Option<&str> {
    // Strip series prefix: everything after the first '-'.
    ticker.split_once('-').map(|(_, rest)| rest)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_and_renders_row() {
        let mut bar = StatusBar::new(100, Duration::from_secs(3));
        bar.set_current("KXBTC15M", "KXBTC15M-26APR161315-00", ConnState::Up);
        bar.mark_data("KXBTC15M");
        bar.set_window_remaining("KXBTC15M", 123);
        bar.set_next("KXBTC15M", Some("KXBTC15M-26APR161330-00"));

        let lines = bar.render();
        assert_eq!(lines.len(), 3); // title + 1 row + bottom
        let row = &lines[1];
        assert!(row.contains("KXBTC15M"));
        assert!(row.contains("UP"));
        assert!(row.contains("LIVE"));
        assert!(row.contains("PRE"));
    }

    #[test]
    fn stale_data_when_old() {
        let mut bar = StatusBar::new(100, Duration::from_millis(1));
        bar.set_current("KXBTC15M", "T", ConnState::Up);
        bar.mark_data("KXBTC15M");
        std::thread::sleep(Duration::from_millis(10));
        let row = &bar.render()[1];
        assert!(row.contains("STALE"));
    }

    #[test]
    fn no_data_when_unseen() {
        let mut bar = StatusBar::new(100, Duration::from_secs(1));
        bar.track("KXBTC15M");
        let row = &bar.render()[1];
        assert!(row.contains("NONE"));
    }

    #[test]
    fn line_count_matches_render() {
        let mut bar = StatusBar::new(100, Duration::from_secs(1));
        bar.track("A");
        bar.track("B");
        assert_eq!(bar.line_count(), bar.render().len());
    }

    #[test]
    fn width_clamped() {
        assert_eq!(StatusBar::new(10, Duration::from_secs(1)).width(), 60);
        assert_eq!(StatusBar::new(999, Duration::from_secs(1)).width(), 140);
        assert_eq!(StatusBar::new(80, Duration::from_secs(1)).width(), 80);
    }
}
