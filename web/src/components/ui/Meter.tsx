/**
 * Ratio against a limit. The fill carries severity (accent → warning →
 * error); the track is a lighter step of the same hue.
 */
export function Meter({ value, label, warnAt = 75, errAt = 90 }: { value: number | null; label: string; warnAt?: number; errAt?: number }) {
  const v = value === null ? 0 : Math.max(0, Math.min(100, value))
  const fill = value === null ? 'bg-muted' : v >= errAt ? 'bg-err' : v >= warnAt ? 'bg-warn' : 'bg-accent'
  const track = value === null ? 'bg-line' : v >= errAt ? 'bg-err/20' : v >= warnAt ? 'bg-warn/20' : 'bg-accent/20'
  return (
    <div
      role="meter"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={value === null ? undefined : Math.round(v)}
      className={`h-2 w-full overflow-hidden rounded-full ${track}`}
    >
      <div className={`h-full rounded-full ${fill}`} style={{ width: `${v}%` }} />
    </div>
  )
}
