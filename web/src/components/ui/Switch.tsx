/** On/off switch. `disabledReason` disables it and explains why (tooltip). */
export function Switch({ checked, onChange, label, disabledReason, busy = false }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabledReason?: string | null; busy?: boolean }) {
  const disabled = Boolean(disabledReason) || busy
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      aria-busy={busy || undefined}
      disabled={disabled}
      title={disabledReason ?? label}
      onClick={() => onChange(!checked)}
      className={`relative inline-flex h-5 w-9 shrink-0 items-center rounded-full border transition-colors disabled:cursor-not-allowed disabled:opacity-45 ${checked ? 'border-accent bg-accent' : 'border-line bg-bg-2'} ${busy ? 'animate-pulse' : ''}`}
    >
      <span className={`inline-block size-3.5 rounded-full bg-white shadow transition-transform ${checked ? 'translate-x-[18px]' : 'translate-x-[2px]'}`} aria-hidden />
    </button>
  )
}
