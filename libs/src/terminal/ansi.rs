/// ANSI escape code primitives — colors, cursor movement, line erasure.

// ── Colors / styles ───────────────────────────────────────────────────────────

pub const RESET: &str = "\x1B[0m";
pub const RED: &str = "\x1B[31m";
pub const GREEN: &str = "\x1B[32m";
pub const BOLD: &str = "\x1B[1m";
pub const DIM: &str = "\x1B[2m";
pub const CYAN: &str = "\x1B[36m";

// ── Cursor ────────────────────────────────────────────────────────────────────

/// Save cursor position (terminal-local, not DECSC).
pub const SAVE: &str = "\x1B[s";
/// Restore previously saved cursor position.
pub const RESTORE: &str = "\x1B[u";
/// Move cursor up `n` lines.
pub fn up(n: usize) -> String {
    format!("\x1B[{n}A")
}

// ── Line ──────────────────────────────────────────────────────────────────────

/// Erase the current line and move to column 1.
pub const ERASE_LINE: &str = "\x1B[2K\r";

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Strip ANSI escape sequences and return the visible display width.
pub fn visible_len(s: &str) -> usize {
    let mut len = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if c == '\x1B' { in_escape = true; continue; }
        if in_escape   { if c == 'm' { in_escape = false; } continue; }
        len += 1;
    }
    len
}
