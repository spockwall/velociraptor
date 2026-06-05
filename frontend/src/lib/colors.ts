//! Centralized color tokens for inline `style={{}}` usage.
//!
//! Single source of truth for color *values* is `src/index.css`'s `@theme`
//! block (the `--color-*` custom properties); Tailwind utility classes
//! (`bg-bg-base`, `text-text-primary`, …) already resolve to those. This module
//! is the matching accessor for the places that need a raw color string in a
//! JS `style` object (charts, dynamic per-status colors, recharts props) so they
//! reference the *same* tokens by name instead of hardcoding hex.
//!
//! Add a color → add it to `@theme` in index.css, then add the matching var()
//! reference here. Never put a hex literal in a component.

export const C = {
    // Surfaces & borders
    bgCard: "var(--color-bg-card)",
    bgChip: "var(--color-bg-chip)",
    borderStrong: "var(--color-border-strong)",
    borderCard: "var(--color-border-card)",
    borderFaint: "var(--color-border-faint)",
    borderChip: "var(--color-border-chip)",

    // Text scale (light → dark)
    textBright: "var(--color-text-bright)",
    textStrong: "var(--color-text-strong)",
    textDim: "var(--color-text-dim)",
    textFaint: "var(--color-text-faint)",
    textSubtle: "var(--color-text-subtle)",
    textGhost: "var(--color-text-ghost)",
    textTrace: "var(--color-text-trace)",

    // Accent / status
    green: "var(--color-accent-green)",
    red: "var(--color-accent-red)",
    blue: "var(--color-accent-blue)",
    gold: "var(--color-accent-gold)",
    amber: "var(--color-accent-amber)",

    // Bright, saturated accent variants (solid action buttons)
    greenBright: "var(--color-accent-green-bright)",
    redBright: "var(--color-accent-red-bright)",
    blueBright: "var(--color-accent-blue-bright)",
    greenBrightHover: "var(--color-accent-green-bright-hover)",
    redBrightHover: "var(--color-accent-red-bright-hover)",
    blueBrightHover: "var(--color-accent-blue-bright-hover)",

    // Error banner
    errorText: "var(--color-error-text)",
    errorBg: "var(--color-error-bg)",
    errorBorder: "var(--color-error-border)",
} as const;
