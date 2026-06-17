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
          <div className="flex items-baseline gap-2 min-w-0">
            <p
              className="text-xs font-semibold capitalize whitespace-nowrap"
              style={{ color: 'var(--color-text-strong)' }}
            >
              {title}
            </p>
            {subtitle && (
              <p className="text-xs truncate" style={{ color: 'var(--color-text-trace)' }}>
                {subtitle}
              </p>
            )}
          </div>
          {action && <div>{action}</div>}
        </div>
      )}
      <div className={noPad ? '' : 'p-4'}>{children}</div>
    </div>
  )
}
