/// Inline fixed-height orderbook visualizer.
///
/// # Module layout
///
/// ```text
/// terminal/
/// ├── mod.rs    — OrderbookUi (public API, draw loop)
/// ├── ansi.rs   — ANSI escape primitives (colors, cursor, erase)
/// ├── layout.rs — terminal size detection, grid arithmetic
/// └── panel.rs  — SymbolSnapshot, PanelRenderer trait, DefaultRenderer
/// ```
///
/// # Extending the display
///
/// Implement [`panel::PanelRenderer`] and pass it to [`OrderbookUi::new_with_renderer`]:
///
/// ```rust,no_run
/// use libs::terminal::{OrderbookUi, panel::{PanelRenderer, SymbolSnapshot}};
///
/// struct MyRenderer;
/// impl PanelRenderer for MyRenderer {
///     fn panel_width(&self) -> usize { 32 }
///     fn panel_height(&self, depth: usize) -> usize { depth + 3 }
///     fn render(&self, snap: &SymbolSnapshot, depth: usize) -> Vec<String> {
///         vec![format!("{}:{} bid={:?}", snap.exchange, snap.symbol, snap.bids.first())]
///     }
/// }
///
/// let mut ui = OrderbookUi::new_with_renderer(5, MyRenderer).unwrap();
/// ```
pub mod ansi;
pub mod layout;
pub mod panel;
pub mod polymarket;
pub mod status;

pub use polymarket::PolymarketUi;
pub use status::{ConnState, NextState, SeriesStatus, StatusBar};

use ansi::{up, ERASE_LINE, RESTORE, SAVE};
use layout::{fit_depth, max_columns, term_size};
use panel::{DefaultRenderer, PanelRenderer, SymbolSnapshot};

use std::collections::HashMap;
use std::io::{self, Write};

// ── OrderbookUi ───────────────────────────────────────────────────────────────

/// Inline side-by-side orderbook panel display.
///
/// Panels are drawn at a fixed position in the normal terminal buffer —
/// no alternate screen, no raw mode. Ctrl+C works via the OS signal.
pub struct OrderbookUi {
    depth: usize,
    renderer: Box<dyn PanelRenderer>,
    /// Insertion-ordered keys → panel order.
    order: Vec<String>,
    snapshots: HashMap<String, SymbolSnapshot>,
    initialized: bool,
    block_height: usize,
}

impl OrderbookUi {
    /// Create a display using the default renderer.
    pub fn new(depth: usize) -> io::Result<Self> {
        Self::new_with_renderer(depth, DefaultRenderer)
    }

    /// Create a display with a custom [`panel::PanelRenderer`].
    pub fn new_with_renderer(
        depth: usize,
        renderer: impl PanelRenderer + 'static,
    ) -> io::Result<Self> {
        Ok(Self {
            depth,
            renderer: Box::new(renderer),
            order: Vec::new(),
            snapshots: HashMap::new(),
            initialized: false,
            block_height: 0,
        })
    }

    /// Update one symbol's snapshot and redraw all panels.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        exchange: &str,
        symbol: &str,
        sequence: u64,
        best_bid: Option<(f64, f64)>,
        best_ask: Option<(f64, f64)>,
        spread: Option<f64>,
        mid: Option<f64>,
        wmid: f64,
        bids: &[(f64, f64)],
        asks: &[(f64, f64)],
    ) {
        let _ = (best_bid, best_ask); // available to callers; not used by default renderer
        let key = format!("{exchange}:{symbol}");
        if !self.snapshots.contains_key(&key) {
            self.order.push(key.clone());
        }
        let depth = self.depth;
        self.snapshots.insert(
            key,
            SymbolSnapshot {
                exchange: exchange.to_string(),
                symbol: symbol.to_string(),
                sequence,
                spread,
                mid,
                wmid,
                bids: bids[..bids.len().min(depth)].to_vec(),
                asks: asks[..asks.len().min(depth)].to_vec(),
            },
        );
        self.draw();
    }

    /// No-op — terminal restores automatically (no raw mode was entered).
    pub fn restore(&mut self) -> io::Result<()> {
        Ok(())
    }

    // ── Internal draw ─────────────────────────────────────────────────────────

    fn draw(&mut self) {
        let n = self.order.len();
        if n == 0 {
            return;
        }

        let (term_cols, term_rows) = term_size();
        let panel_width = self.renderer.panel_width();
        let cols = n.min(max_columns(term_cols, panel_width, 2));
        let grid_rows = n.div_ceil(cols);
        let depth = self.depth.min(fit_depth(term_rows, grid_rows, 4, 2).max(1));
        let panel_height = self.renderer.panel_height(depth);
        let total_height = grid_rows * panel_height;

        let mut out = Vec::with_capacity(8192);

        if !self.initialized {
            for _ in 0..total_height {
                out.extend_from_slice(b"\n");
            }
            out.extend_from_slice(up(total_height).as_bytes());
            out.extend_from_slice(SAVE.as_bytes());
            self.initialized = true;
            self.block_height = total_height;
        } else if total_height != self.block_height {
            out.extend_from_slice(RESTORE.as_bytes());
            if total_height > self.block_height {
                let extra = total_height - self.block_height;
                for _ in 0..extra {
                    out.extend_from_slice(b"\n");
                }
                out.extend_from_slice(up(total_height).as_bytes());
            }
            out.extend_from_slice(SAVE.as_bytes());
            self.block_height = total_height;
        } else {
            out.extend_from_slice(RESTORE.as_bytes());
            out.extend_from_slice(SAVE.as_bytes());
        }

        // Build all panel line buffers
        let all_panels: Vec<Vec<String>> = self
            .order
            .iter()
            .map(|key| {
                if let Some(snap) = self.snapshots.get(key) {
                    self.renderer.render(snap, depth)
                } else {
                    vec![" ".repeat(panel_width); panel_height]
                }
            })
            .collect();

        // Render grid row by row
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

        let stdout = io::stdout();
        let mut lock = stdout.lock();
        let _ = lock.write_all(&out);
        let _ = lock.flush();
    }
}
