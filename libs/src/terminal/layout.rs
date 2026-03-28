/// Terminal size detection and grid layout calculation.

use crossterm::terminal::size as terminal_size;

/// Live terminal dimensions, falling back to 80×24 when not a TTY.
pub fn term_size() -> (usize, usize) {
    let (cols, rows) = terminal_size().unwrap_or((80, 24));
    (cols as usize, rows as usize)
}

/// How many panels fit per row given the terminal width.
///
/// - `panel_width` — visible character width of one panel
/// - `gap`         — visible character gap between panels
pub fn max_columns(term_cols: usize, panel_width: usize, gap: usize) -> usize {
    ((term_cols + gap) / (panel_width + gap)).max(1)
}

/// Maximum depth (levels per side) that fits vertically.
///
/// Panel height formula: `depth * 2 + overhead` lines.
/// `overhead` = title + header + spread row + bottom border = 4 lines.
pub fn fit_depth(term_rows: usize, grid_rows: usize, overhead: usize, reserved: usize) -> usize {
    let available = term_rows.saturating_sub(reserved);
    let lines_per_panel = available / grid_rows;
    (lines_per_panel.saturating_sub(overhead)) / 2
}
