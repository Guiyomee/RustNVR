import { Shield, ShieldOff } from 'lucide-react'
import { useEffect, useState } from 'react'
import { describeError } from '../../api/client'
import { protectRange, unprotectRange } from '../../api/endpoints'
import { formatDateTime, formatDuration } from '../../lib/format'
import type { ProtectedRange } from '../../types/api'
import { Button } from '../ui/Button'
import { Card, CardHeader } from '../ui/Card'
import { ErrorState, LoadingState } from '../ui/States'
import { Modal } from '../ui/Modal'
import { fromTimeInput, toTimeInput } from './ExportDialog'
import { t } from '../../lib/i18n'

const MAX_MS = 31 * 86_400_000

interface DialogProps {
  open: boolean
  onClose: () => void
  onProtected: () => void
  cameras: { id: string; name: string }[]
  initialCamera: string | null
  /** Day shown on the page (local midnight). */
  day: number
  /** Playback position: start of the period by default. */
  position: number | null
}

/** Lecture → « Protéger » : keeps a period whatever the retention. */
export function ProtectDialog({ open, onClose, onProtected, cameras, initialCamera, day, position }: DialogProps) {
  const [camera, setCamera] = useState(initialCamera ?? '')
  const [start, setStart] = useState('')
  const [end, setEnd] = useState('')
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Fresh values each time the window opens: one minute before and after the position.
  useEffect(() => {
    if (!open) return
    const at = position ?? day
    setCamera(initialCamera ?? cameras[0]?.id ?? '')
    setStart(toTimeInput(Math.max(day, at - 60_000)))
    setEnd(toTimeInput(Math.min(at + 60_000, day + 86_400_000 - 1000)))
    setNote('')
    setError(null)
  }, [open])

  const fromMs = fromTimeInput(day, start)
  const toMs = fromTimeInput(day, end)
  const span = fromMs !== null && toMs !== null ? toMs - fromMs : null
  const invalid = !camera ? t('Choisissez une caméra', 'Choose a camera') : span === null ? t('Heures invalides', 'Invalid times') : span <= 0 ? t('La fin doit être après le début', 'The end must be after the start') : span > MAX_MS ? t('31 jours au plus', '31 days at most') : null

  const run = async () => {
    if (invalid || fromMs === null || toMs === null) return
    setBusy(true)
    setError(null)
    try {
      await protectRange({ camera, startMs: fromMs, endMs: toMs, note: note.trim() })
      onProtected()
      onClose()
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
      title={t('Protéger un enregistrement', 'Protect a recording')}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            {t('Annuler', 'Cancel')}
          </Button>
          <Button variant="primary" icon={<Shield className="size-4" />} disabledReason={busy ? t('Enregistrement…', 'Saving…') : invalid} onClick={() => void run()}>
            {t('Protéger', 'Protect')}
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
          {position !== null && (
            <div className="flex flex-wrap gap-1">
              <Button size="sm" disabled={busy} onClick={() => setStart(toTimeInput(position))}>
                {t('Début = position', 'Start = position')}
              </Button>
              <Button size="sm" disabled={busy} onClick={() => setEnd(toTimeInput(position))}>
                {t('Fin = position', 'End = position')}
              </Button>
            </div>
          )}
        </div>
        <label className="block">
          <span className="mb-1 block text-xs text-muted">{t('Note (facultative)', 'Note (optional)')}</span>
          <input className={`${input} w-full`} maxLength={200} placeholder={t('Ex. : livraison du colis, voiture abîmée…', 'E.g.: parcel delivery, damaged car…')} value={note} onChange={(e) => setNote(e.target.value)} disabled={busy} />
        </label>
        {span !== null && span > 0 && !invalid && <p className="text-xs text-muted">{t('Durée protégée : ', 'Protected length: ')}{formatDuration(span)}.</p>}
        {error && <p className="text-err">{error}</p>}
        <p className="text-xs text-muted">
          {t('Les enregistrements de cette période ne seront jamais effacés, ni par la rétention, ni quand la place manque, jusqu’à « Ne plus protéger ». Ils restent sur chaque point de stockage où ils existent.', 'The recordings of this period will never be deleted, neither by retention nor when space runs out, until « Stop protecting ». They stay on each storage destination where they exist.')}
        </p>
      </div>
    </Modal>
  )
}

/** Protected periods, with « Lire » and « Ne plus protéger ». */
export function ProtectedList({
  list,
  error,
  onReload,
  names,
  readOnly,
  onPlay,
}: {
  list: ProtectedRange[] | null
  error: unknown
  onReload: () => void
  names: (id: string) => string
  readOnly: string | null
  onPlay: (p: ProtectedRange) => void
}) {
  const [busy, setBusy] = useState<number | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  const remove = async (id: number) => {
    setBusy(id)
    setFailure(null)
    try {
      await unprotectRange(id)
      onReload()
    } catch (e) {
      setFailure(describeError(e))
    } finally {
      setBusy(null)
    }
  }
  return (
    <Card>
      <CardHeader title={t('Enregistrements protégés', 'Protected recordings')} subtitle={t('Jamais effacés par la rétention ni par manque de place', 'Never deleted by retention nor by lack of space')} icon={<Shield className="size-4" />} />
      {error && !list ? (
        <ErrorState compact error={error} onRetry={onReload} />
      ) : !list ? (
        <LoadingState compact />
      ) : list.length === 0 ? (
        <p className="px-4 py-3 text-sm text-muted">{t('Aucun. Le bouton « Protéger » garde un passage important au-delà de la rétention.', 'None. The « Protect » button keeps an important moment beyond the retention.')}</p>
      ) : (
        <ul className="divide-y divide-line">
          {list.map((p) => (
            <li key={p.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2 text-sm">
              <span className="font-medium">{names(p.camera)}</span>
              <span className="tabular">
                {formatDateTime(p.start_ms)} → {formatDateTime(p.end_ms)}
              </span>
              <span className="text-xs text-muted">{formatDuration(p.end_ms - p.start_ms)}</span>
              {p.note && <span className="text-muted">« {p.note} »</span>}
              <span className="ml-auto flex gap-2">
                <Button size="sm" onClick={() => onPlay(p)}>
                  {t('Lire', 'Play')}
                </Button>
                <Button size="sm" icon={<ShieldOff className="size-3.5" />} disabledReason={readOnly ?? (busy === p.id ? t('En cours…', 'In progress…') : null)} onClick={() => void remove(p.id)}>
                  {t('Ne plus protéger', 'Stop protecting')}
                </Button>
              </span>
            </li>
          ))}
        </ul>
      )}
      {failure && <p className="px-4 pb-3 text-sm text-err">{failure}</p>}
    </Card>
  )
}
