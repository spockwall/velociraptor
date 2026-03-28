/// Panel rendering — converts a `SymbolSnapshot` into a fixed-width line buffer.
///
/// To add a new panel layout, implement the `PanelRenderer` trait and pass it
/// to `OrderbookUi::new_with_renderer`.

use super::ansi::*;

// ── Snapshot data ─────────────────────────────────────────────────────────────

/// All data required to render one orderbook panel.
#[derive(Clone)]
pub struct SymbolSnapshot {
    pub exchange: String,
    pub symbol: String,
    pub sequence: u64,
    pub spread: Option<f64>,
    pub mid: Option<f64>,
    pub wmid: f64,
    pub bids: Vec<(f64, f64)>,
    pub asks: Vec<(f64, f64)>,
}

// ── Renderer trait ────────────────────────────────────────────────────────────

/// Trait for building a panel's line buffer from a snapshot.
///
/// Implement this to customise how a panel looks without touching the rest
/// of the display logic.
///
/// ```rust,no_run
/// use libs::terminal::panel::{PanelRenderer, SymbolSnapshot};
///
/// pub struct CompactRenderer;
///
/// impl PanelRenderer for CompactRenderer {
///     fn panel_width(&self) -> usize { 32 }
///     fn panel_height(&self, depth: usize) -> usize { depth * 2 + 3 }
///     fn render(&self, snap: &SymbolSnapshot, depth: usize) -> Vec<String> {
///         // ... build lines ...
///         vec![]
///     }
/// }
/// ```
pub trait PanelRenderer: Send + Sync {
    /// Visible character width of one panel (including borders).
    fn panel_width(&self) -> usize;
    /// Total number of lines this renderer emits per panel.
    fn panel_height(&self, depth: usize) -> usize;
    /// Build the panel as a list of ANSI-formatted strings.
    fn render(&self, snap: &SymbolSnapshot, depth: usize) -> Vec<String>;
}

// ── Default renderer ──────────────────────────────────────────────────────────

/// Standard orderbook panel: asks (red) above bids (green) with a spread row.
pub struct DefaultRenderer;

impl PanelRenderer for DefaultRenderer {
    fn panel_width(&self) -> usize { 46 }

    fn panel_height(&self, depth: usize) -> usize {
        depth * 2 + 4 // title + header + asks + spread + bids + bottom
    }

    fn render(&self, snap: &SymbolSnapshot, depth: usize) -> Vec<String> {
        build_panel_lines(snap, depth, self.panel_width())
    }
}

// ── Panel builder ─────────────────────────────────────────────────────────────

/// Right-align `text` in a field of `width` visible columns.
pub fn rpad(text: &str, width: usize) -> String {
    let vlen = visible_len(text);
    if vlen >= width {
        text.to_string()
    } else {
        format!("{}{}", " ".repeat(width - vlen), text)
    }
}

/// Repeated `─` for horizontal borders (`n` display columns).
pub fn hline(n: usize) -> String {
    "─".repeat(n)
}

/// One full-width content row: `│ c1  c2  c3 │`
pub fn content_row(color: &str, c1: &str, c2: &str, c3: &str, w1: usize, w2: usize, w3: usize) -> String {
    format!(
        "│{color}{}{RESET}  {color}{}{RESET}  {color}{}{RESET}│",
        rpad(c1, w1), rpad(c2, w2), rpad(c3, w3),
        color = color,
    )
}

/// Build the default panel line buffer.
fn build_panel_lines(snap: &SymbolSnapshot, depth: usize, width: usize) -> Vec<String> {
    let inner = width.saturating_sub(2);
    let w1 = 14usize; // price
    let w2 = 14usize; // qty
    let w3 = inner.saturating_sub(w1 + w2 + 4); // label

    let mut lines = Vec::new();

    // ── Title ─────────────────────────────────────────────────────────────────
    let title_plain = format!(
        " {}:{}  seq={}  wmid={:.2} ",
        snap.exchange, snap.symbol, snap.sequence, snap.wmid,
    );
    let title_styled = format!(
        " {BOLD}{CYAN}{}:{}{RESET}  seq={DIM}{}{RESET}  wmid={:.2} ",
        snap.exchange, snap.symbol, snap.sequence, snap.wmid,
    );
    let pad = inner.saturating_sub(visible_len(&title_plain));
    lines.push(format!("┌{}{}┐", title_styled, hline(pad)));

    // ── Header ────────────────────────────────────────────────────────────────
    lines.push(content_row(DIM, "price", "qty", "label", w1, w2, w3));

    // ── Asks: worst → best ────────────────────────────────────────────────────
    for (price, qty) in snap.asks.iter().rev() {
        lines.push(content_row(RED, &format!("{price:.4}"), &format!("{qty:.6}"), "ask", w1, w2, w3));
    }
    for _ in snap.asks.len()..depth {
        lines.push(content_row(DIM, "─", "─", "ask", w1, w2, w3));
    }

    // ── Spread / mid separator ────────────────────────────────────────────────
    let spread_str = snap.spread.map(|v| format!("{v:.4}")).unwrap_or_else(|| "─".into());
    let mid_str    = snap.mid.map(|v| format!("{v:.4}")).unwrap_or_else(|| "─".into());
    let esc_extra = visible_len(BOLD) + visible_len(RESET);
    lines.push(format!(
        "├{}  {}  {}┤",
        rpad(&format!("{BOLD}{spread_str}{RESET}"), w1 + esc_extra),
        rpad(&format!("{BOLD}{mid_str}{RESET}"),    w2 + esc_extra),
        rpad(&format!("{BOLD}sprd/mid{RESET}"),     w3 + esc_extra),
    ));

    // ── Bids: best → worst ────────────────────────────────────────────────────
    for (price, qty) in &snap.bids {
        lines.push(content_row(GREEN, &format!("{price:.4}"), &format!("{qty:.6}"), "bid", w1, w2, w3));
    }
    for _ in snap.bids.len()..depth {
        lines.push(content_row(DIM, "─", "─", "bid", w1, w2, w3));
    }

    // ── Bottom border ─────────────────────────────────────────────────────────
    lines.push(format!("└{}┘", hline(inner)));

    lines
}
