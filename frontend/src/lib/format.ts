export function fmtPrice(v: number | null | undefined, decimals = 4): string {
  if (v == null) return '—'
  return v.toLocaleString('en-US', { minimumFractionDigits: decimals, maximumFractionDigits: decimals })
}

export function fmtQty(v: number | null | undefined, decimals = 2): string {
  if (v == null) return '—'
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(2)}M`
  if (v >= 1_000) return `${(v / 1_000).toFixed(2)}K`
  return v.toFixed(decimals)
}

export function fmtTs(iso: string): string {
  return new Date(iso).toLocaleTimeString('en-US', { hour12: false })
}

export function fmtSpread(v: number | null | undefined): string {
  if (v == null) return '—'
  return v.toFixed(6)
}

export function fmtPct(v: number): string {
  return `${(v * 100).toFixed(2)}%`
}

export function relativeTime(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime()
  if (diff < 1000) return 'just now'
  if (diff < 60_000) return `${Math.floor(diff / 1000)}s ago`
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`
  return `${Math.floor(diff / 3_600_000)}h ago`
}
