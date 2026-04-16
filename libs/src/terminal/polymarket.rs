//! Polymarket-specific terminal panel renderer.
//!
//! Renders Up and Down orderbooks as a single unified panel per market slug.
//! Each panel is split into left (Up/Yes) and right (Down/No) columns.
//!
//! ```text
//! ┌ btc-updown-15m-1775XXXXXX ───────────────────────────────────────────┐
//! │         UP (Yes)          │           DOWN (No)                      │
//! │  price          qty       │   price          qty                     │
//! │  0.5300      1234.00  ask │   0.4800      567.00  ask                │
//! │  0.5200       890.00  ask │   0.4900      234.00  ask                │
//! ├──────── sprd=0.0100 ──────┼──────── sprd=0.0200 ──────────────────── ┤
//! │  0.5100       345.00  bid │   0.5000      678.00  bid                │
//! │  0.5000       123.00  bid │   0.4700      901.00  bid                │
//! └──────────────────────────────────────────────────────────────────────┘
//! ```

use super::ansi::*;
use super::layout::{fit_depth, max_columns, term_size};
use super::panel::{hline, rpad};
use std::collections::HashMap;
use std::io::{self, Write};

// ── Data ──────────────────────────────────────────────────────────────────────

/// One side (Up or Down) of a Polymarket market's orderbook.
#[derive(Clone, Default)]
pub struct Side {
    pub sequence: u64,
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
    pub spread: Option<f64>,
    pub mid: Option<f64>,
}

/// A paired Up+Down snapshot for one Polymarket slug.
#[derive(Clone, Default)]
pub struct MarketPair {
    pub slug: String,         // base slug (panel key, e.g. "btc-updown-5m")
    pub display_slug: String, // latest full slug (e.g. "btc-updown-5m-1775308500")
    pub up: Side,
    pub down: Side,
}

// ── UI ────────────────────────────────────────────────────────────────────────

/// Terminal display for Polymarket paired markets.
///
/// Each market slug occupies one panel showing Up (left) and Down (right) books.
pub struct PolymarketUi {
    depth: usize,
    order: Vec<String>, // slug insertion order
    pairs: HashMap<String, MarketPair>,
    initialized: bool,
    block_height: usize,
    /// Extra lines rendered above the orderbook panels (e.g. a status bar).
    status_lines: Vec<String>,
}

impl PolymarketUi {
    pub fn new(depth: usize) -> Self {
        Self {
            depth,
            order: Vec::new(),
            pairs: HashMap::new(),
            initialized: false,
            block_height: 0,
            status_lines: Vec::new(),
        }
    }

    /// Set the lines rendered above the orderbook panels. Typically populated
    /// with [`super::status::StatusBar::render`] output each tick. Pass an
    /// empty vec to remove.
    pub fn set_status_lines(&mut self, lines: Vec<String>) {
        self.status_lines = lines;
    }

    /// Update one side of a market and redraw.
    /// `slug`         — base slug used as panel key (e.g. `"btc-updown-5m"`)
    /// `display_slug` — full timestamped slug shown in the panel title (e.g. `"btc-updown-5m-1775308500"`)
    /// `is_up`        — true = Up/Yes side, false = Down/No side
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        slug: &str,
        display_slug: &str,
        is_up: bool,
        sequence: u64,
        bids: Vec<(f64, f64)>,
        asks: Vec<(f64, f64)>,
        spread: Option<f64>,
        mid: Option<f64>,
    ) {
        self.set_side(slug, display_slug, is_up, sequence, bids, asks, spread, mid);
        self.draw();
    }

    /// Update one side without redrawing. Batch multiple `set_side` calls and
    /// finish with a single `render()` to avoid redrawing N× per tick.
    #[allow(clippy::too_many_arguments)]
    pub fn set_side(
        &mut self,
        slug: &str,
        display_slug: &str,
        is_up: bool,
        sequence: u64,
        bids: Vec<(f64, f64)>,
        asks: Vec<(f64, f64)>,
        spread: Option<f64>,
        mid: Option<f64>,
    ) {
        self.insert_pair_if_absent(slug, display_slug);
        let pair = self.pairs.get_mut(slug).expect("pair exists after insert");
        pair.display_slug = display_slug.to_string();
        let side = if is_up { &mut pair.up } else { &mut pair.down };
        side.sequence = sequence;
        side.bids = bids;
        side.asks = asks;
        side.spread = spread;
        side.mid = mid;
    }

    /// Redraw the terminal from current state. Pair with `set_side` for batching.
    pub fn render(&mut self) {
        self.draw();
    }

    /// Returns the current panel key slugs in display order.
    pub fn panels(&self) -> Vec<String> {
        self.order.clone()
    }

    /// Remove a panel by its key slug (the `full_slug` used as the panel key).
    /// Triggers a redraw. No-op if the slug is not present.
    pub fn remove(&mut self, slug: &str) {
        if self.pairs.remove(slug).is_some() {
            self.order.retain(|s| s != slug);
            self.draw();
        }
    }

    /// Ensure a panel exists for `slug`, creating an empty placeholder if needed.
    /// Use this to make the successor window visible immediately after the prior one expires.
    /// No-op if the panel already has data.
    pub fn ensure(&mut self, slug: &str) {
        if self.pairs.contains_key(slug) {
            return;
        }
        self.insert_pair_if_absent(slug, slug);
        self.draw();
    }

    fn insert_pair_if_absent(&mut self, slug: &str, display_slug: &str) {
        if self.pairs.contains_key(slug) {
            return;
        }
        self.order.push(slug.to_string());
        self.pairs.insert(
            slug.to_string(),
            MarketPair {
                slug: slug.to_string(),
                display_slug: display_slug.to_string(),
                ..Default::default()
            },
        );
        sort_order(&mut self.order);
    }

    // ── Drawing ───────────────────────────────────────────────────────────────

    fn draw(&mut self) {
        let n = self.order.len();
        if n == 0 && self.status_lines.is_empty() {
            return;
        }

        let (term_cols, term_rows) = term_size();

        // Panel width: each panel is split half/half. Min viable width = 60.
        let panel_width = panel_width(term_cols, n.max(1));
        let cols = n.max(1).min(max_columns(term_cols, panel_width, 2));
        let grid_rows = if n == 0 { 0 } else { n.div_ceil(cols) };
        let depth = self.depth.min(fit_depth(term_rows, grid_rows.max(1), 5, 2).max(1));
        let panel_height = depth * 2 + 5; // title + header + asks + spread + bids + bottom
        let status_height = self.status_lines.len();
        let total_height = status_height + grid_rows * panel_height;

        let mut out = Vec::with_capacity(16384);

        let old_block_height = self.block_height;

        if !self.initialized {
            for _ in 0..total_height {
                out.extend_from_slice(b"\n");
            }
            out.extend_from_slice(up(total_height).as_bytes());
            out.extend_from_slice(SAVE.as_bytes());
            self.initialized = true;
            self.block_height = total_height;
        } else if total_height > self.block_height {
            // Grow: push down extra lines so SAVE has room, then reposition.
            out.extend_from_slice(RESTORE.as_bytes());
            let extra = total_height - self.block_height;
            for _ in 0..extra {
                out.extend_from_slice(b"\n");
            }
            out.extend_from_slice(up(total_height).as_bytes());
            out.extend_from_slice(SAVE.as_bytes());
            self.block_height = total_height;
        } else {
            // Same size or shrink: just restore to saved position.
            out.extend_from_slice(RESTORE.as_bytes());
            out.extend_from_slice(SAVE.as_bytes());
            self.block_height = total_height;
        }

        // ── Status lines (above the panels) ───────────────────────────────────
        for line in &self.status_lines {
            out.extend_from_slice(ERASE_LINE.as_bytes());
            out.extend_from_slice(line.as_bytes());
            out.extend_from_slice(b"\n");
        }

        let all_panels: Vec<Vec<String>> = self
            .order
            .iter()
            .map(|slug| {
                if let Some(pair) = self.pairs.get(slug) {
                    render_pair(pair, depth, panel_width)
                } else {
                    vec![" ".repeat(panel_width); panel_height]
                }
            })
            .collect();

        for grid_row in 0..grid_rows {
            let start = grid_row * cols;
            let end = (start + cols).min(n);
            let slice = &all_panels[start..end];

            for line_idx in 0..panel_height {
                out.extend_from_slice(ERASE_LINE.as_bytes());
                for (i, panel) in slice.iter().enumerate() {
                    let line = panel.get(line_idx).map(String::as_str).unwrap_or("");
                    out.extend_from_slice(line.as_bytes());
                    if i + 1 < slice.len() {
                        out.extend_from_slice(b"  ");
                    }
                }
                out.extend_from_slice(b"\n");
            }
        }

        // Erase any lines that belonged to the previous (taller) block.
        let tail = old_block_height.saturating_sub(total_height);
        for _ in 0..=tail {
            out.extend_from_slice(ERASE_LINE.as_bytes());
            out.extend_from_slice(b"\n");
        }

        let stdout = io::stdout();
        let mut lock = stdout.lock();
        let _ = lock.write_all(&out);
        let _ = lock.flush();
    }
}

// ── Panel ordering ────────────────────────────────────────────────────────────

/// Sort panel keys: primary = trailing timestamp (ascending), secondary = slug prefix (alpha).
/// Slugs without a numeric suffix sort before timestamped ones (static markets first).
fn sort_order(order: &mut [String]) {
    order.sort_by(|a, b| {
        let ts_a = trailing_timestamp(a);
        let ts_b = trailing_timestamp(b);
        ts_a.cmp(&ts_b).then_with(|| a.cmp(b))
    });
}

/// Extract the trailing Unix timestamp from a slug, if present.
fn trailing_timestamp(slug: &str) -> Option<u64> {
    slug.rsplit('-').next().and_then(|s| s.parse::<u64>().ok())
}

// ── Panel width ───────────────────────────────────────────────────────────────

/// Choose a panel width that fits the terminal, shared across all panels.
fn panel_width(term_cols: usize, n_panels: usize) -> usize {
    // Try to fit all panels side by side, with a 2-char gap, min 60, max 120.
    let ideal = if n_panels == 0 {
        term_cols.min(120)
    } else {
        ((term_cols + 2) / n_panels).saturating_sub(2)
    };
    ideal.clamp(60, 120)
}

// ── Rendering ─────────────────────────────────────────────────────────────────

fn render_pair(pair: &MarketPair, depth: usize, width: usize) -> Vec<String> {
    let inner = width.saturating_sub(2); // inside the border chars
    let half = inner / 2;
    let right_w = inner.saturating_sub(half + 1);
    let w_price = 8usize;
    let w_qty = 10usize;

    let mut lines = Vec::new();

    // ── Title ─────────────────────────────────────────────────────────────────
    let title_text = if pair.display_slug.is_empty() {
        &pair.slug
    } else {
        &pair.display_slug
    };
    let title_plain = format!(" {} ", title_text);
    let title_styled = format!(" {BOLD}{CYAN}{}{RESET} ", title_text);
    let pad = inner.saturating_sub(visible_len(&title_plain));
    lines.push(format!("┌{}{}┐", title_styled, hline(pad)));

    // ── Sub-header: UP | DOWN ─────────────────────────────────────────────────
    let up_hdr = centre("UP  (Yes)", half);
    let down_hdr = centre("DOWN  (No)", right_w);
    lines.push(format!(
        "│{BOLD}{GREEN}{up_hdr}{RESET}│{BOLD}{RED}{down_hdr}{RESET}│"
    ));

    // ── Column headers ────────────────────────────────────────────────────────
    let col_hdr = format!("{}{}{}", rpad("price", w_price), rpad("qty", w_qty), "  ");
    let up_col = truncate(&col_hdr, half);
    let down_col = truncate(&col_hdr, right_w);
    lines.push(format!("│{DIM}{up_col}{RESET}│{DIM}{down_col}{RESET}│"));

    // ── Asks: worst → best ────────────────────────────────────────────────────
    let up_asks: Vec<_> = pair.up.asks.iter().rev().copied().collect();
    let down_asks: Vec<_> = pair.down.asks.iter().rev().copied().collect();
    for i in 0..depth {
        let left = side_row(up_asks.get(i).copied(), RED, half, w_price, w_qty);
        let right = side_row(down_asks.get(i).copied(), RED, right_w, w_price, w_qty);
        lines.push(format!("│{left}│{right}│"));
    }

    // ── Spread / mid separator ────────────────────────────────────────────────
    let up_label = spread_mid_label(pair.up.spread, pair.up.mid);
    let down_label = spread_mid_label(pair.down.spread, pair.down.mid);
    let left_sep = format!(
        "{}{}{}",
        hline(2),
        up_label,
        hline(half.saturating_sub(2 + visible_len(&up_label)))
    );
    let right_sep = format!(
        "{}{}{}",
        hline(2),
        down_label,
        hline(right_w.saturating_sub(2 + visible_len(&down_label)))
    );
    lines.push(format!("├{left_sep}┼{right_sep}┤"));

    // ── Bids: best → worst ────────────────────────────────────────────────────
    for i in 0..depth {
        let left = side_row(pair.up.bids.get(i).copied(), GREEN, half, w_price, w_qty);
        let right = side_row(
            pair.down.bids.get(i).copied(),
            GREEN,
            right_w,
            w_price,
            w_qty,
        );
        lines.push(format!("│{left}│{right}│"));
    }

    // ── Bottom ────────────────────────────────────────────────────────────────
    lines.push(format!("└{}┘", hline(inner)));

    lines
}

/// One level row for one side: `  price      qty  `
fn side_row(
    level: Option<(f64, f64)>,
    color: &str,
    width: usize,
    w_price: usize,
    w_qty: usize,
) -> String {
    let (price_s, qty_s) = match level {
        Some((p, q)) => (format!("{p:.4}"), format!("{q:.4}")),
        None => ("──────".into(), "──────".into()),
    };
    let row = format!("{}{}{}", rpad(&price_s, w_price), rpad(&qty_s, w_qty), "  ");
    format!("{color}{}{RESET}", truncate(&row, width))
}

/// Text shown in the spread/mid divider row for one side.
fn spread_mid_label(spread: Option<f64>, mid: Option<f64>) -> String {
    let sprd = spread
        .map(|v| format!("sprd={v:.4}"))
        .unwrap_or_else(|| "──────".into());
    let mid_s = mid.map(|v| format!("mid={v:.4}")).unwrap_or_default();
    if mid_s.is_empty() {
        sprd
    } else {
        format!("{sprd} {mid_s}")
    }
}

// ── String helpers ────────────────────────────────────────────────────────────

fn centre(text: &str, width: usize) -> String {
    let vlen = visible_len(text);
    if vlen >= width {
        return text[..width.min(text.len())].to_string();
    }
    let left = (width - vlen) / 2;
    let right = width - vlen - left;
    format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
}

/// Truncate or pad a string to exactly `width` visible columns.
fn truncate(s: &str, width: usize) -> String {
    let vlen = visible_len(s);
    if vlen < width {
        format!("{}{}", s, " ".repeat(width - vlen))
    } else {
        // Clip at the visible boundary.
        let mut out = String::new();
        let mut count = 0;
        let mut in_esc = false;
        for c in s.chars() {
            if c == '\x1B' {
                in_esc = true;
                out.push(c);
                continue;
            }
            if in_esc {
                out.push(c);
                if c == 'm' {
                    in_esc = false;
                }
                continue;
            }
            if count >= width {
                break;
            }
            out.push(c);
            count += 1;
        }
        out
    }
}
