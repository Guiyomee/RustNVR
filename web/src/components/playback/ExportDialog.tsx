import { Download } from 'lucide-react'
import { useEffect, useState } from 'react'
import { describeError } from '../../api/client'
import { exportClip } from '../../api/endpoints'
import { formatBytes, formatDuration } from '../../lib/format'
import type { ExportResult } from '../../types/api'
import { Button } from '../ui/Button'
import { Modal } from '../ui/Modal'
import { decodesHevc } from './RecordingPlayer'
import { t } from '../../lib/i18n'

const MAX_MS = 2 * 3_600_000
const LENGTHS = [
  { ms: 30_000, label: '30 s' },
  { ms: 60_000, label: '1 min' },
  { ms: 5 * 60_000, label: '5 min' },
  { ms: 15 * 60_000, label: '15 min' },
]

/** `HH:MM:SS` of a time, for <input type="time" step="1">. */
export function toTimeInput(ms: number): string {
  const d = new Date(ms)
  const p = (n: number) => String(n).padStart(2, '0')
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`
}

/** Time of the day `day` given as `HH:MM[:SS]`. */
export function fromTimeInput(day: number, v: string): number | null {
  const m = /^(\d{2}):(\d{2})(?::(\d{2}))?$/.exec(v)
  if (!m) return null
  const d = new Date(day)
  d.setHours(Number(m[1]), Number(m[2]), Number(m[3] ?? 0), 0)
  return d.getTime()
}

interface Props {
  open: boolean
  onClose: () => void
  cameras: { id: string; name: string; codec: string | null }[]
  /** Camera chosen first. */
  initialCamera: string | null
  /** Day shown on the page (local midnight). */
  day: number
  /** Playback position: start of the extract by default. */
  position: number | null
}

/** Lecture → « Exporter » : one MP4 file of the chosen period. */
export function ExportDialog({ open, onClose, cameras, initialCamera, day, position }: Props) {
  const [camera, setCamera] = useState(initialCamera ?? '')
  const [start, setStart] = useState('')
  const [end, setEnd] = useState('')
  const [h264, setH264] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [result, setResult] = useState<ExportResult | null>(null)
  const codec = cameras.find((c) => c.id === camera)?.codec ?? null

  // Fresh values each time the window opens: from the current position, one minute.
  useEffect(() => {
    if (!open) return
    const from = position ?? day
    setCamera(initialCamera ?? cameras[0]?.id ?? '')
    setStart(toTimeInput(from))
    setEnd(toTimeInput(Math.min(from + 60_000, day + 86_400_000 - 1000)))
    setError(null)
    setResult(null)
  }, [open])

  // A browser without H.265 could not read the copied file.
  useEffect(() => setH264(codec === 'h265' && !decodesHevc()), [codec])

  const fromMs = fromTimeInput(day, start)
  const toMs = fromTimeInput(day, end)
  const span = fromMs !== null && toMs !== null ? toMs - fromMs : null
  const invalid =
    !camera ? t('Choisissez une caméra', 'Choose a camera') : span === null ? t('Heures invalides', 'Invalid times') : span <= 0 ? t('La fin doit être après le début', 'The end must be after the start') : span > MAX_MS ? t('2 heures au plus par extrait', '2 hours at most per clip') : null

  const run = async () => {
    if (invalid || fromMs === null || toMs === null) return
    setBusy(true)
    setError(null)
    setResult(null)
    try {
      const r = await exportClip(camera, { fromMs, toMs, h264 })
      setResult(r)
      // Starts the download right away; the link stays for another try.
      const a = document.createElement('a')
      a.href = r.url
      a.download = r.name
      document.body.append(a)
      a.click()
      a.remove()
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }

  const input = 'h-9 rounded-md border border-line bg-bg-2 px-2 text-sm tabular'
  return (
    <Modal
      open={open}
      title={t('Exporter un extrait', 'Export a clip')}
      onClose={busy ? () => undefined : onClose}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            {t('Fermer', 'Close')}
          </Button>
          <Button variant="primary" icon={<Download className="size-4" />} disabledReason={busy ? t('Préparation en cours…', 'Preparing…') : invalid} onClick={() => void run()}>
            {busy ? t('Préparation…', 'Preparing…') : t('Exporter', 'Export')}
          </Button>
        </>
      }
    >
      <div className="space-y-4 text-sm">
        <label className="block">
          <span className="mb-1 block text-xs text-muted">{t('Caméra', 'Camera')}</span>
          <select className={`${input} w-full`} value={camera} onChange={(e) => setCamera(e.target.value)} disabled={busy}>
            {cameras.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </label>
        <div className="flex flex-wrap items-end gap-3">
          <label>
            <span className="mb-1 block text-xs text-muted">{t('Début', 'Start')}</span>
            <input type="time" step={1} className={input} value={start} onChange={(e) => setStart(e.target.value)} disabled={busy} />
          </label>
          <label>
            <span className="mb-1 block text-xs text-muted">{t('Fin', 'End')}</span>
            <input type="time" step={1} className={input} value={end} onChange={(e) => setEnd(e.target.value)} disabled={busy} />
          </label>
          <div className="flex flex-wrap gap-1">
            {LENGTHS.map((l) => (
              <Button
                key={l.ms}
                size="sm"
                disabled={busy || fromMs === null}
                onClick={() => fromMs !== null && setEnd(toTimeInput(Math.min(fromMs + l.ms, day + 86_400_000 - 1000)))}
              >
                {l.label}
              </Button>
            ))}
          </div>
        </div>
        {position !== null && (
          <div className="flex flex-wrap gap-2">
            <Button size="sm" disabled={busy} onClick={() => setStart(toTimeInput(position))}>
              {t('Début = position actuelle', 'Start = current position')}
            </Button>
            <Button size="sm" disabled={busy} onClick={() => setEnd(toTimeInput(position))}>
              {t('Fin = position actuelle', 'End = current position')}
            </Button>
          </div>
        )}
        <label className="flex items-start gap-2">
          <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={h264} onChange={(e) => setH264(e.target.checked)} disabled={busy} />
          <span>
            {t('Convertir en H.264', 'Convert to H.264')}
            <span className="block text-xs text-muted">
              {codec === 'h265'
                ? t('Caméra en H.265 : à cocher pour un fichier lisible partout (navigateur sans H.265, téléphone, messagerie). Plus long à préparer.', 'H.265 camera: tick it for a file that plays everywhere (browser without H.265, phone, messaging). Longer to prepare.')
                : t('Inutile pour une caméra en H.264 : sans conversion, le fichier est une copie exacte et il est prêt en quelques secondes.', 'Not needed for an H.264 camera: without conversion, the file is an exact copy and is ready in a few seconds.')}
            </span>
          </span>
        </label>
        {span !== null && span > 0 && !invalid && <p className="text-xs text-muted">{t('Durée demandée : ', 'Requested length: ')}{formatDuration(span)}{t('. Les périodes sans enregistrement sont sautées.', '. Periods without recording are skipped.')}</p>}
        {busy && <p className="text-muted">{t('Préparation du fichier', 'Preparing the file')}{h264 ? t(' (la conversion en H.264 prend plus de temps)', ' (the conversion to H.264 takes longer)') : ''}…</p>}
        {error && <p className="text-err">{error}</p>}
        {result && (
          <p>
            {t('Prêt :', 'Ready:')}{' '}
            <a className="text-accent underline" href={result.url} download={result.name}>
              {result.name}
            </a>{' '}
            <span className="text-muted">
              ({formatDuration(result.duration_ms)}, {formatBytes(result.bytes)}{t('). Le lien reste valable 15 minutes.', '). The link stays valid for 15 minutes.')}
            </span>
          </p>
        )}
        <p className="text-xs text-muted">{t('Sans conversion, l’image et le son sont copiés tels quels (aucune perte) ; l’extrait commence à l’image clé précédant l’heure de début.', 'Without conversion, picture and sound are copied as they are (no loss); the clip starts at the key frame before the start time.')}</p>
      </div>
    </Modal>
  )
}
