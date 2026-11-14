import { Bookmark as BookmarkIcon, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { describeError } from '../../api/client'
import { addBookmark, deleteBookmark } from '../../api/endpoints'
import { formatDateTime, formatTime } from '../../lib/format'
import type { Bookmark } from '../../types/api'
import { Button } from '../ui/Button'
import { Card, CardHeader } from '../ui/Card'
import { ErrorState, LoadingState } from '../ui/States'
import { Modal } from '../ui/Modal'
import { t } from '../../lib/i18n'

const MAX_NOTE = 200

/** Lecture → « Signet » : marks the playback position with a note. */
export function BookmarkDialog({
  open,
  onClose,
  onAdded,
  cameras,
  initialCamera,
  position,
}: {
  open: boolean
  onClose: () => void
  onAdded: () => void
  cameras: { id: string; name: string }[]
  initialCamera: string | null
  position: number | null
}) {
  const [camera, setCamera] = useState(initialCamera ?? '')
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!open) return
    setCamera(initialCamera ?? cameras[0]?.id ?? '')
    setNote('')
    setError(null)
  }, [open])

  const invalid = position === null ? t('Aucune position de lecture', 'No playback position') : !camera ? t('Choisissez une caméra', 'Choose a camera') : !note.trim() ? t('Écrivez une note', 'Write a note') : null

  const run = async () => {
    if (invalid || position === null) return
    setBusy(true)
    setError(null)
    try {
      await addBookmark({ camera, atMs: position, note: note.trim() })
      onAdded()
      onClose()
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }

  const input = 'h-9 rounded-md border border-line bg-bg-2 px-2 text-sm'
  return (
    <Modal
      open={open}
      title={t('Ajouter un signet', 'Add a bookmark')}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose} disabled={busy}>
            {t('Annuler', 'Cancel')}
          </Button>
          <Button variant="primary" icon={<BookmarkIcon className="size-4" />} disabledReason={busy ? t('Enregistrement…', 'Saving…') : invalid} onClick={() => void run()}>
            {t('Ajouter', 'Add')}
          </Button>
        </>
      }
    >
      <form
        className="space-y-4 text-sm"
        onSubmit={(e) => {
          e.preventDefault()
          void run()
        }}
      >
        <p>
          {t('Moment : ', 'Moment: ')}
          <span className="tabular font-medium">{position !== null ? formatDateTime(position) : '—'}</span>
        </p>
        {cameras.length > 1 && (
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
        )}
        <label className="block">
          <span className="mb-1 block text-xs text-muted">{t('Note', 'Note')}</span>
          <input autoFocus className={`${input} w-full`} maxLength={MAX_NOTE} placeholder={t('Ex. : livraison colis, facteur, voisin…', 'E.g.: parcel delivery, postman, neighbour…')} value={note} onChange={(e) => setNote(e.target.value)} disabled={busy} />
        </label>
        {error && <p className="text-err">{error}</p>}
        <p className="text-xs text-muted">{t('Le signet apparaît sur la timeline et dans la liste du jour, pour tous les appareils. Il ne protège pas la vidéo de la rétention (bouton « Protéger »).', 'The bookmark shows on the timeline and in the list of the day, on every device. It does not protect the video from retention (« Protect » button).')}</p>
      </form>
    </Modal>
  )
}

/** Bookmarks of the day shown, with « Lire » and delete. */
export function BookmarkList({
  list,
  error,
  onReload,
  names,
  readOnly,
  onPlay,
}: {
  list: Bookmark[] | null
  error: unknown
  onReload: () => void
  names: (id: string) => string
  readOnly: string | null
  onPlay: (b: Bookmark) => void
}) {
  const [busy, setBusy] = useState<number | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  const remove = async (id: number) => {
    setBusy(id)
    setFailure(null)
    try {
      await deleteBookmark(id)
      onReload()
    } catch (e) {
      setFailure(describeError(e))
    } finally {
      setBusy(null)
    }
  }
  return (
    <Card>
      <CardHeader title={t('Signets du jour', 'Bookmarks of the day')} subtitle={t('Les moments notés sur ces caméras', 'Moments noted on these cameras')} icon={<BookmarkIcon className="size-4" />} />
      {error && !list ? (
        <ErrorState compact error={error} onRetry={onReload} />
      ) : !list ? (
        <LoadingState compact />
      ) : list.length === 0 ? (
        <p className="px-4 py-3 text-sm text-muted">{t('Aucun. Le bouton « Signet » note le moment en cours de lecture (« livraison colis ») pour le retrouver sans tout relire.', 'None. The « Bookmark » button notes the moment being played (« parcel delivery ») to find it again without watching everything.')}</p>
      ) : (
        <ul className="divide-y divide-line">
          {list.map((b) => (
            <li key={b.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2 text-sm">
              <span className="tabular font-medium">{formatTime(b.at_ms, true)}</span>
              <span className="text-muted">{names(b.camera)}</span>
              <span className="min-w-0 flex-1 break-words">{b.note}</span>
              {b.author && <span className="text-xs text-muted">{b.author}</span>}
              <span className="flex gap-2">
                <Button size="sm" onClick={() => onPlay(b)}>
                  {t('Lire', 'Play')}
                </Button>
                <Button size="sm" icon={<Trash2 className="size-3.5" />} disabledReason={readOnly ?? (busy === b.id ? t('En cours…', 'In progress…') : null)} onClick={() => void remove(b.id)}>
                  {t('Supprimer', 'Delete')}
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
