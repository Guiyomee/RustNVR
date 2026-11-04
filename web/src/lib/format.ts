// Formatting helpers (locale of the chosen language).
import { locale, t } from './i18n'

const nf1 = new Intl.NumberFormat(locale(), { maximumFractionDigits: 1 })
const nf0 = new Intl.NumberFormat(locale(), { maximumFractionDigits: 0 })

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined) return '—'
  const units = [t('o', 'B'), t('Ko', 'KB'), t('Mo', 'MB'), t('Go', 'GB'), t('To', 'TB'), t('Po', 'PB')]
  let v = bytes
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${i === 0 ? nf0.format(v) : nf1.format(v)} ${units[i]}`
}

export function formatPercent(v: number | null | undefined): string {
  return v === null || v === undefined ? '—' : `${nf0.format(v)} %`
}

export function formatNumber(v: number | null | undefined, digits = 0): string {
  if (v === null || v === undefined) return '—'
  return new Intl.NumberFormat(locale(), { maximumFractionDigits: digits }).format(v)
}

export function formatBitrate(kbps: number | null | undefined): string {
  if (kbps === null || kbps === undefined) return '—'
  return kbps >= 1000 ? `${nf1.format(kbps / 1000)} Mbit/s` : `${nf0.format(kbps)} kbit/s`
}

export function formatDuration(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return '—'
  const s = Math.round(ms / 1000)
  const d = Math.floor(s / 86400)
  const h = Math.floor((s % 86400) / 3600)
  const m = Math.floor((s % 3600) / 60)
  const sec = s % 60
  if (d > 0) return t(`${d} j ${h} h`, `${d} d ${h} h`)
  if (h > 0) return `${h} h ${String(m).padStart(2, '0')} min`
  if (m > 0) return `${m} min ${String(sec).padStart(2, '0')} s`
  return `${sec} s`
}

export function formatTime(ms: number, withSeconds = false): string {
  return new Date(ms).toLocaleTimeString(locale(), {
    hour: '2-digit',
    minute: '2-digit',
    ...(withSeconds ? { second: '2-digit' } : {}),
  })
}

export function formatDate(ms: number): string {
  return new Date(ms).toLocaleDateString(locale(), { day: '2-digit', month: 'short', year: 'numeric' })
}

export function formatDateTime(ms: number): string {
  return `${formatDate(ms)} ${formatTime(ms, true)}`
}

export function formatResolution(w: number | null, h: number | null): string {
  return w && h ? `${w}×${h}` : '—'
}

/** Local midnight of the day containing `ms`. */
export function startOfDay(ms: number): number {
  const d = new Date(ms)
  d.setHours(0, 0, 0, 0)
  return d.getTime()
}

/** `YYYY-MM-DD` of the local day containing `ms` (for <input type="date">). */
export function toDateInput(ms: number): string {
  const d = new Date(ms)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`
}

export function fromDateInput(v: string): number | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(v)
  if (!m) return null
  return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])).getTime()
}

/** Recording destinations of a camera, for display (`local` → disque local). */
export function destinationsLabel(ids: string[]): string {
  if (ids.length === 0) return t('disque local', 'local disk')
  return ids.map((id) => (id === 'local' ? t('disque local', 'local disk') : id)).join(' + ')
}
