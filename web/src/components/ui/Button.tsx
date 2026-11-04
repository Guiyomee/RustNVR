import type { ButtonHTMLAttributes, ReactNode } from 'react'

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger'

const styles: Record<Variant, string> = {
  primary: 'bg-accent text-white hover:bg-accent/90 border-transparent',
  secondary: 'bg-card text-fg border-line hover:bg-line/60',
  ghost: 'bg-transparent text-muted border-transparent hover:text-fg hover:bg-line/50',
  danger: 'bg-err/15 text-err border-err/40 hover:bg-err/25',
}

interface Props extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant
  size?: 'sm' | 'md'
  icon?: ReactNode
  /**
   * When set, the button is disabled and this text explains why (tooltip and
   * accessible description). Every inactive control must say why.
   */
  disabledReason?: string | null
}

export function Button({ variant = 'secondary', size = 'md', icon, disabledReason, className = '', children, title, ...rest }: Props) {
  const disabled = Boolean(disabledReason) || rest.disabled
  const pad = size === 'sm' ? 'h-8 px-2.5 text-xs gap-1.5' : 'h-9 px-3.5 text-sm gap-2'
  const label = children ?? null
  return (
    <button
      type="button"
      {...rest}
      disabled={disabled}
      title={disabledReason ?? title}
      aria-description={disabledReason ?? undefined}
      className={`inline-flex shrink-0 items-center justify-center rounded-md border font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${pad} ${styles[variant]} ${className}`}
    >
      {icon}
      {label}
    </button>
  )
}

/** Square icon-only button; `label` is required for screen readers. */
export function IconButton({ label, icon, disabledReason, className = '', ...rest }: Omit<Props, 'children'> & { label: string; icon: ReactNode }) {
  const disabled = Boolean(disabledReason) || rest.disabled
  return (
    <button
      type="button"
      {...rest}
      disabled={disabled}
      aria-label={label}
      title={disabledReason ? `${label} — ${disabledReason}` : label}
      className={`inline-flex size-8 items-center justify-center rounded-md text-muted transition-colors hover:bg-line/60 hover:text-fg disabled:cursor-not-allowed disabled:opacity-40 ${className}`}
    >
      {icon}
    </button>
  )
}
