import { Plus, Save, Send, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { describeError } from '../../api/client'
import { getWebhooks, setWebhooks, testWebhook } from '../../api/endpoints'
import { useAuth, useReadOnly } from '../../auth/AuthGate'
import { useIsDemo, useLive } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { OBJECT_GROUPS, objectLabel } from '../../lib/objects'
import type { Webhook } from '../../types/api'
import { Button } from '../ui/Button'
import { ErrorState, LoadingState } from '../ui/States'
import { Switch } from '../ui/Switch'
import { t } from '../../lib/i18n'

const inputCls = 'mt-1 h-9 w-full rounded-md border border-line bg-bg-2 px-3 text-sm text-fg outline-none focus:border-accent disabled:opacity-60'

const EVENTS: { key: Webhook['events'][number]; label: string; help: string }[] = [
  { key: 'motion', label: t('Mouvement', 'Motion'), help: t('avec l’image, ≈ 1 s après le début', 'with the picture, ≈ 1 s after the start') },
  { key: 'objects', label: t('Objets reconnus', 'Recognized objects'), help: t('IA optionnelle, image encadrée', 'optional AI, picture with boxes') },
  { key: 'end', label: t('Fin de l’événement', 'End of the event'), help: t('durée et objets, sans image', 'length and objects, no picture') },
  { key: 'crossing', label: t('Passage de ligne', 'Line crossing'), help: t('ligne de passage franchie (avec l’objet vu sur la ligne)', 'crossing line crossed (with the object seen at the line)') },
  { key: 'storage', label: t('Stockage dégradé', 'Storage degraded'), help: t('partage non monté, en lecture seule ou qui ne répond plus ; puis rétabli', 'share not mounted, read-only or not answering; then back') },
  { key: 'camera', label: t('Caméra hors ligne / rétablie', 'Camera offline / back'), help: t('plus d’image depuis 1 min, puis de nouveau en ligne', 'no picture for 1 min, then online again') },
]

/** Objects offered as filters: the usual ones (people, vehicles, animals). */
const FILTER_OBJECTS = OBJECT_GROUPS.flatMap((g) => g.labels).slice(0, 24)

const blank = (): Webhook => ({ name: '', url: '', enabled: true, cameras: [], objects: [], events: ['motion', 'objects'], image: true })

/** Same rule as `WebhookConfig::is_discord` on the server. */
function isDiscord(url: string): boolean {
  return /^https?:\/\/(?:[^/@]*@)?(?:(?:ptb|canary)\.)?discord(?:app)?\.com(?::\d+)?\/api\/webhooks\//i.test(url.trim())
}

function toggle<T>(list: T[], v: T): T[] {
  return list.includes(v) ? list.filter((x) => x !== v) : [...list, v]
}

function Chip({ on, label, onClick, disabled }: { on: boolean; label: string; onClick: () => void; disabled: boolean }) {
  return (
    <button
      type="button"
      aria-pressed={on}
      disabled={disabled}
      onClick={onClick}
      className={`rounded-full border px-2.5 py-0.5 text-xs transition-colors disabled:opacity-50 ${on ? 'border-accent bg-accent/15 text-fg' : 'border-line text-muted hover:text-fg'}`}
    >
      {label}
    </button>
  )
}

/** One webhook being edited. */
function HookEditor({ hook, onChange, onRemove, disabled }: { hook: Webhook; onChange: (h: Webhook) => void; onRemove: () => void; disabled: boolean }) {
  const cameras = useLive((s) => s.cameras)
  const [trying, setTrying] = useState(false)
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null)
  const set = <K extends keyof Webhook>(k: K, v: Webhook[K]) => onChange({ ...hook, [k]: v })
  const tryIt = async () => {
    setTrying(true)
    setResult(null)
    try {
      const r = await testWebhook(hook)
      setResult(r.error ? { ok: false, text: t(`Échec : ${r.error} (${r.ms} ms)`, `Failed: ${r.error} (${r.ms} ms)`) } : { ok: true, text: t(`Réponse ${r.status} en ${r.ms} ms`, `Answer ${r.status} in ${r.ms} ms`) })
    } catch (e) {
      setResult({ ok: false, text: describeError(e) })
    } finally {
      setTrying(false)
    }
  }
  const urlOk = /^https?:\/\/.{3,}/.test(hook.url)
  const discord = isDiscord(hook.url)
  return (
    <div className="space-y-3 rounded-md border border-line p-4">
      <div className="flex flex-wrap items-end gap-3">
        <label className="w-48 text-sm">
          {t('Nom', 'Name')}
          <input className={inputCls} value={hook.name} disabled={disabled} placeholder="n8n" onChange={(e) => set('name', e.target.value)} />
        </label>
        <label className="min-w-72 flex-1 text-sm">
          {t('Adresse appelée (POST)', 'Address called (POST)')}
          <input className={inputCls} value={hook.url} disabled={disabled} placeholder="https://n8n.maison/webhook/…" onChange={(e) => set('url', e.target.value)} />
          {discord && <span className="mt-1 block text-xs text-ok">{t('Webhook Discord : envoyé comme un message Discord (titre, objets, zones, passages, image jointe).', 'Discord webhook: sent as a Discord message (title, objects, zones, crossings, attached picture).')}</span>}
        </label>
        <Switch checked={hook.enabled} onChange={(v) => set('enabled', v)} label={t('Actif', 'Active')} disabledReason={disabled ? t('Lecture seule', 'Read only') : null} />
      </div>
      <div className="space-y-1 text-sm">
        <span className="text-xs text-muted">{t('Appelé pour', 'Called for')}</span>
        <div className="flex flex-wrap gap-4">
          {EVENTS.map((e) => (
            <label key={e.key} className="flex items-start gap-2">
              <input type="checkbox" className="mt-0.5 size-4 accent-accent" disabled={disabled} checked={hook.events.includes(e.key)} onChange={() => set('events', toggle(hook.events, e.key))} />
              <span>
                {e.label}
                <span className="block text-xs text-muted">{e.help}</span>
              </span>
            </label>
          ))}
          <label className="flex items-start gap-2">
            <input type="checkbox" className="mt-0.5 size-4 accent-accent" disabled={disabled} checked={hook.image} onChange={(e) => set('image', e.target.checked)} />
            <span>
              {t('Joindre l’image', 'Attach the picture')}
              <span className="block text-xs text-muted">{discord ? t('JPEG joint au message', 'JPEG attached to the message') : t('JPEG en base64 dans le JSON', 'JPEG in base64 inside the JSON')}</span>
            </span>
          </label>
        </div>
      </div>
      <div className="space-y-1">
        <span className="text-xs text-muted">{t('Caméras (aucune cochée : toutes)', 'Cameras (none ticked: all)')}</span>
        <div className="flex flex-wrap gap-1.5">
          {cameras.map((c) => (
            <Chip key={c.id} on={hook.cameras.includes(c.id)} label={c.name} disabled={disabled} onClick={() => set('cameras', toggle(hook.cameras, c.id))} />
          ))}
        </div>
      </div>
      <div className="space-y-1">
        <span className="text-xs text-muted">{t('Objets (aucun coché : tous les événements ; sinon seulement les objets reconnus cochés, sans les simples mouvements ni les passages sans objet)', 'Objects (none ticked: every event; otherwise only the ticked recognized objects, without plain motion nor crossings without object)')}</span>
        <div className="flex flex-wrap gap-1.5">
          {FILTER_OBJECTS.map((l) => (
            <Chip key={l} on={hook.objects.includes(l)} label={objectLabel(l)} disabled={disabled} onClick={() => set('objects', toggle(hook.objects, l))} />
          ))}
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" icon={<Send className="size-3.5" />} disabledReason={disabled ? t('Lecture seule', 'Read only') : !urlOk ? t('Adresse http:// ou https:// attendue', 'http:// or https:// address expected') : trying ? t('Appel en cours…', 'Calling…') : null} onClick={() => void tryIt()}>
          {t('Essayer', 'Try')}
        </Button>
        <Button size="sm" icon={<Trash2 className="size-3.5" />} disabledReason={disabled ? t('Lecture seule', 'Read only') : null} onClick={onRemove}>
          {t('Supprimer', 'Delete')}
        </Button>
        {result && <span className={`text-xs ${result.ok ? 'text-ok' : 'text-err'}`}>{result.text}</span>}
      </div>
    </div>
  )
}

/** Paramètres → Webhooks. */
export function WebhooksSettings() {
  const demo = useIsDemo()
  const { user } = useAuth()
  const readOnly = useReadOnly()
  const admin = !user || user.role === 'admin'
  const loaded = useAsync(() => (demo || !admin ? Promise.resolve(null) : getWebhooks()), [demo, admin])
  const [hooks, setHooks] = useState<Webhook[] | null>(null)
  const [saved, setSaved] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [done, setDone] = useState(false)
  useEffect(() => {
    if (loaded.data) {
      setHooks(loaded.data)
      setSaved(JSON.stringify(loaded.data))
    }
  }, [loaded.data])

  if (demo) return <p className="px-5 py-4 text-sm text-muted">{t('Indisponible en mode démo.', 'Not available in demo mode.')}</p>
  if (!admin) return <p className="px-5 py-4 text-sm text-muted">{t('Les webhooks sont réservés aux administrateurs (leurs adresses peuvent contenir un jeton).', 'Webhooks are for administrators only (their addresses may contain a token).')}</p>
  if (loaded.error && !hooks) return <ErrorState compact error={loaded.error} onRetry={loaded.reload} />
  if (!hooks) return <LoadingState compact />

  const disabled = readOnly !== null || busy
  const changed = JSON.stringify(hooks) !== saved
  const problem = hooks.some((h) => !h.name.trim()) ? t('Chaque webhook doit avoir un nom', 'Each webhook needs a name') : hooks.some((h) => !/^https?:\/\/.{3,}/.test(h.url)) ? t('Adresse http:// ou https:// attendue', 'http:// or https:// address expected') : hooks.some((h) => h.events.length === 0) ? t('Cochez au moins un type d’appel', 'Tick at least one kind of call') : null
  const save = async () => {
    setBusy(true)
    setError(null)
    setDone(false)
    try {
      const next = await setWebhooks(hooks.map((h) => ({ ...h, name: h.name.trim(), url: h.url.trim() })))
      setHooks(next)
      setSaved(JSON.stringify(next))
      setDone(true)
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-4 px-5 py-4">
      {hooks.length === 0 && <p className="text-sm text-muted">{t('Aucun webhook. Ajoutez une adresse à appeler (n8n, Node-RED, Home Assistant, script…).', 'No webhook. Add an address to call (n8n, Node-RED, Home Assistant, script…).')}</p>}
      {hooks.map((h, i) => (
        <HookEditor key={i} hook={h} disabled={disabled} onChange={(next) => setHooks(hooks.map((x, j) => (j === i ? next : x)))} onRemove={() => setHooks(hooks.filter((_, j) => j !== i))} />
      ))}
      <div className="flex flex-wrap items-center gap-2">
        <Button icon={<Plus className="size-4" />} disabledReason={disabled ? t('Lecture seule', 'Read only') : hooks.length >= 20 ? t('20 webhooks au plus', '20 webhooks at most') : null} onClick={() => setHooks([...hooks, blank()])}>
          {t('Ajouter un webhook', 'Add a webhook')}
        </Button>
        <Button variant="primary" icon={<Save className="size-4" />} disabledReason={readOnly ?? (busy ? t('Enregistrement…', 'Saving…') : !changed ? t('Aucun changement', 'No change') : problem)} onClick={() => void save()}>
          {t('Enregistrer', 'Save')}
        </Button>
        {done && !changed && <span className="text-xs text-ok">{t('Enregistré, appliqué tout de suite.', 'Saved, applied at once.')}</span>}
        {error && <span className="text-sm text-err">{error}</span>}
      </div>
      <details className="text-xs text-muted">
        <summary className="cursor-pointer">{t('Contenu envoyé', 'Content sent')}</summary>
        <pre className="mt-2 overflow-auto rounded bg-bg-2 p-3 font-mono whitespace-pre-wrap">{`POST <adresse>  (Content-Type: application/json)
{ "type": "motion" | "objects" | "end" | "test",
  "camera": "jardin", "camera_name": "Jardin",
  "event": { "id": 42, "start_ms": …, "end_ms": null, "score": 0.31,
             "zones": ["Allée"], "objects": [{ "label": "person", "score": 0.87, "bbox": [x1, y1, x2, y2], "zone": "Allée" }] },
  "image": "data:image/jpeg;base64,…" | null,
  "rustnvr": "2026.11.1" }`}</pre>
        <p className="mt-2">{t('Adresse Discord (discord.com/api/webhooks/…) : un message Discord à la place de ce JSON, que Discord refuse.', 'Discord address (discord.com/api/webhooks/…): a Discord message instead of this JSON, which Discord refuses.')}</p>
        {t('Un appel sans réponse en 10 s ou en erreur est retenté une fois après 5 s. Les adresses ne sont jamais écrites en entier dans le journal (seulement le nom de la machine).', 'A call without answer within 10 s or failing is tried once more after 5 s. Addresses are never written in full in the log (only the machine name).')}
      </details>
    </div>
  )
}
