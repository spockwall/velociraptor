import type { ReactNode } from 'react'

interface CardProps {
  title?: string
  subtitle?: string
  action?: ReactNode
  children: ReactNode
  className?: string
  noPad?: boolean
}

export default function Card({ title, subtitle, action, children, className = '', noPad }: CardProps) {
  return (
    <div
      className={`rounded border ${className}`}
      style={{ background: 'var(--color-bg-card)', borderColor: 'var(--color-border-card)' }}
    >
      {(title || action) && (
        <div
          className="flex items-center justify-between px-4 py-2.5 border-b"
          style={{ borderColor: 'var(--color-border-card)' }}
        >
          <div>
            <p className="text-xs font-medium" style={{ color: 'var(--color-text-strong)' }}>{title}</p>
            {subtitle && (
              <p className="text-xs mt-0.5" style={{ color: 'var(--color-text-trace)' }}>{subtitle}</p>
            )}
          </div>
          {action && <div>{action}</div>}
        </div>
      )}
      <div className={noPad ? '' : 'p-4'}>{children}</div>
    </div>
  )
}
