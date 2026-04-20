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
      style={{ background: '#141414', borderColor: '#1f1f1f' }}
    >
      {(title || action) && (
        <div
          className="flex items-center justify-between px-4 py-2.5 border-b"
          style={{ borderColor: '#1f1f1f' }}
        >
          <div>
            <p className="text-xs font-medium" style={{ color: '#c0c0c0' }}>{title}</p>
            {subtitle && (
              <p className="text-xs mt-0.5" style={{ color: '#444' }}>{subtitle}</p>
            )}
          </div>
          {action && <div>{action}</div>}
        </div>
      )}
      <div className={noPad ? '' : 'p-4'}>{children}</div>
    </div>
  )
}
