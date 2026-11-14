import { Plus, Save, Send, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { describeError } from '../../api/client'
import { getWebhooks, setWebhooks, testWebhook } from '../../api/endpoints'
import { useAuth, useReadOnly } from '../../auth/AuthGate'
import { useIsDemo, useLive } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { OBJECT_GROUPS, objectLabel } from '../../lib/objects'
import type { Webhook, WebhookEvent, WebhookKind, WebhookMessage } from '../../types/api'
import { Button } from '../ui/Button'
import { ErrorState, LoadingState } from '../ui/States'
import { ScheduleEditor } from '../ui/ScheduleEditor'
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
  { key: 'recording', label: t('Erreur d’enregistrement', 'Recording error'), help: t('écriture impossible (disque plein, erreur du disque…) ; puis rétabli au segment suivant', 'cannot write (disk full, disk error…); then back at the next segment') },
]

/** Objects offered as filters: the usual ones (people, vehicles, animals). */
const FILTER_OBJECTS = OBJECT_GROUPS.flatMap((g) => g.labels).slice(0, 24)

const blank = (): Webhook => ({ name: '', url: '', kind: 'json', enabled: true, cameras: [], objects: [], events: ['motion', 'objects'], image: true })

/** Same rule as `WebhookConfig::is_discord` on the server. */
function isDiscord(url: string): boolean {
  return /^https?:\/\/(?:[^/@]*@)?(?:(?:ptb|canary)\.)?discord(?:app)?\.com(?::\d+)?\/api\/webhooks\//i.test(url.trim())
}

/** The kind of a webhook (as the server: a Discord address without kind is Discord). */
function kindOf(h: Webhook): WebhookKind {
  return h.kind ?? (isDiscord(h.url) ? 'discord' : 'json')
}

const KINDS: { key: WebhookKind; label: string; help: string }[] = [
  { key: 'json', label: t('Webhook (JSON)', 'Webhook (JSON)'), help: t('n8n, Node-RED, Home Assistant, script… : l’événement en JSON.', 'n8n, Node-RED, Home Assistant, script…: the event as JSON.') },
  { key: 'discord', label: 'Discord', help: t('Un message dans un salon : Paramètres du salon → Intégrations → Webhooks → Nouveau webhook, puis « Copier l’URL ».', 'A message in a channel: channel settings → Integrations → Webhooks → New webhook, then « Copy webhook URL ».') },
  { key: 'ntfy', label: 'ntfy', help: t('Une notification sur le téléphone (appli ntfy), depuis ntfy.sh ou votre propre serveur ntfy. Abonnez-vous au même sujet dans l’appli.', 'A notification on the phone (ntfy app), from ntfy.sh or your own ntfy server. Subscribe to the same topic in the app.') },
  { key: 'telegram', label: 'Telegram', help: t('Un message d’un bot : créez-le avec @BotFather (il donne le jeton), écrivez-lui une fois, puis donnez l’identifiant de la conversation (@userinfobot le donne, ou getUpdates).', 'A message from a bot: create it with @BotFather (it gives the token), write to it once, then give the chat identifier (@userinfobot gives it, or getUpdates).') },
]

/** Longest custom title or text, and longest wait (same limits as the server). */
const TEMPLATE_CHARS = 500
const COOLDOWN_MAX = 1440

/** Default title and text of each kind of call (shown as examples), and its variables. */
const MESSAGE_HELP: Record<WebhookEvent, { title: string; text: string; vars: string[] }> = {
  motion: { title: 'Mouvement · {camera}', text: 'Zones : {zones}', vars: ['camera', 'objet', 'zones', 'heure', 'date'] },
  objects: { title: '{objet} · {camera}', text: '{objets}\nZones : {zones}', vars: ['camera', 'objet', 'objets', 'zones', 'heure', 'date'] },
  end: { title: 'Fin de l’événement · {camera}', text: '{objets}\nDurée : {duree}', vars: ['camera', 'objet', 'objets', 'zones', 'duree', 'heure', 'date'] },
  crossing: { title: '{objet} · {ligne} · {camera}', text: 'Ligne « {ligne} » franchie {sens}.\nAujourd’hui : {compte}', vars: ['camera', 'ligne', 'sens', 'compte', 'objet', 'heure', 'date'] },
  storage: { title: 'Stockage {etat} · {stockage}', text: '{erreur}', vars: ['stockage', 'etat', 'erreur', 'heure', 'date'] },
  camera: { title: 'Caméra {etat} · {camera}', text: 'Sans image depuis {duree}. {erreur}', vars: ['camera', 'etat', 'erreur', 'duree', 'heure', 'date'] },
  recording: { title: 'Enregistrement {etat} · {camera}', text: '{erreur}', vars: ['camera', 'etat', 'erreur', 'echecs', 'duree', 'heure', 'date'] },
}

const PRIORITIES: { value: string; label: string }[] = [
  { value: '', label: t('Par défaut', 'Default') },
  { value: '1', label: t('1 · Minimale (sans notification)', '1 · Min (no notification)') },
  { value: '2', label: t('2 · Basse', '2 · Low') },
  { value: '3', label: t('3 · Normale', '3 · Normal') },
  { value: '4', label: t('4 · Haute', '4 · High') },
  { value: '5', label: t('5 · Urgente', '5 · Urgent') },
]

/** The messages worth saving: blank fields and empty messages left out. */
function cleanMessages(h: Webhook): Webhook['messages'] {
  const out: Partial<Record<WebhookEvent, WebhookMessage>> = {}
  for (const [key, m] of Object.entries(h.messages ?? {}) as [WebhookEvent, WebhookMessage | undefined][]) {
    if (!m || !h.events.includes(key)) continue
    const title = (m.title ?? '').trim()
    const text = (m.text ?? '').trim()
    const kept: WebhookMessage = {}
    if (title) kept.title = title
    if (text) kept.text = text
    if (m.priority) kept.priority = m.priority
    if (m.silent) kept.silent = true
    if (Object.keys(kept).length > 0) out[key] = kept
  }
  return out
}

/** Why `h` cannot be saved or tried yet (same checks as the server), or null. */
function hookProblem(h: Webhook): string | null {
  const wait = h.cooldown_minutes ?? 0
  if (!Number.isInteger(wait) || wait < 0 || wait > COOLDOWN_MAX) return t(`Délai entre deux notifications : de 0 à ${COOLDOWN_MAX} minutes`, `Wait between two notifications: 0 to ${COOLDOWN_MAX} minutes`)
  const long = Object.values(h.messages ?? {}).some((m) => (m?.title ?? '').length > TEMPLATE_CHARS || (m?.text ?? '').length > TEMPLATE_CHARS)
  if (long) return t(`${TEMPLATE_CHARS} caractères au plus par titre ou texte`, `${TEMPLATE_CHARS} characters at most per title or text`)
  const kind = kindOf(h)
  if (kind === 'telegram') {
    if (!/^\d+:[A-Za-z0-9_-]{20,}$/.test((h.token ?? '').trim())) return t('Jeton du bot attendu (123456789:AA…, donné par @BotFather)', 'Bot token expected (123456789:AA…, given by @BotFather)')
    if (!/^(-?\d+|@[A-Za-z0-9_]{4,})$/.test((h.chat_id ?? '').trim())) return t('Identifiant de conversation attendu (un nombre, ou @canal)', 'Chat identifier expected (a number, or @channel)')
    return null
  }
  if (!/^https?:\/\/.{3,}/.test(h.url.trim())) return t('Adresse http:// ou https:// attendue', 'http:// or https:// address expected')
  if (kind === 'discord' && !isDiscord(h.url)) return t('Adresse de webhook Discord attendue (https://discord.com/api/webhooks/…)', 'Discord webhook address expected (https://discord.com/api/webhooks/…)')
  return null
}

/** Title, text and delivery chosen for the kinds of call of a message webhook. */
function MessagesEditor({ hook, kind, onChange, disabled }: { hook: Webhook; kind: WebhookKind; onChange: (h: Webhook) => void; disabled: boolean }) {
  const chosen = EVENTS.filter((e) => hook.events.includes(e.key))
  const setMessage = (key: WebhookEvent, change: Partial<WebhookMessage>) =>
    onChange({ ...hook, messages: { ...(hook.messages ?? {}), [key]: { ...(hook.messages?.[key] ?? {}), ...change } } })
  const custom = chosen.filter((e) => {
    const m = hook.messages?.[e.key]
    return Boolean(m && ((m.title ?? '').trim() || (m.text ?? '').trim() || m.priority || m.silent))
  }).length
  return (
    <details className="rounded-md border border-line px-3 py-2 text-sm" open={custom > 0 ? true : undefined}>
      <summary className="cursor-pointer text-xs text-muted">
        {t('Personnaliser les messages', 'Customize the messages')}
        {custom > 0 && t(` (${custom} personnalisé${custom > 1 ? 's' : ''})`, ` (${custom} customized)`)}
      </summary>
      <p className="mt-2 text-xs text-muted">
        {t(
          'Laissés vides, le titre et le texte gardent le message habituel (montré en exemple). Les variables entre accolades sont remplacées : {camera}, {heure}… ; « {etat} » vaut par exemple « hors ligne » puis « rétablie ».',
          'Left blank, the title and the text keep the usual message (shown as an example). The variables between braces are replaced: {camera}, {heure}…; « {etat} » is for instance « hors ligne » then « rétablie ».',
        )}
      </p>
      {chosen.length === 0 && <p className="mt-2 text-xs text-muted">{t('Cochez d’abord les types d’appel.', 'Tick the kinds of call first.')}</p>}
      <div className="mt-2 space-y-3">
        {chosen.map((e) => {
          const m = hook.messages?.[e.key] ?? {}
          const help = MESSAGE_HELP[e.key]
          return (
            <fieldset key={e.key} className="space-y-2 rounded-md border border-line p-3">
              <legend className="px-1 text-xs font-medium">{e.label}</legend>
              <label className="block text-xs text-muted">
                {t('Titre', 'Title')}
                <input className={inputCls} value={m.title ?? ''} disabled={disabled} placeholder={help.title} maxLength={TEMPLATE_CHARS} onChange={(ev) => setMessage(e.key, { title: ev.target.value })} />
              </label>
              <label className="block text-xs text-muted">
                {t('Texte', 'Text')}
                <textarea className={`${inputCls} h-16 py-1.5`} value={m.text ?? ''} disabled={disabled} placeholder={help.text} maxLength={TEMPLATE_CHARS} onChange={(ev) => setMessage(e.key, { text: ev.target.value })} />
              </label>
              <div className="flex flex-wrap items-end gap-4">
                {kind === 'ntfy' && (
                  <label className="w-56 text-xs text-muted">
                    {t('Priorité ntfy', 'ntfy priority')}
                    <select className={inputCls} value={m.priority ? String(m.priority) : ''} disabled={disabled} onChange={(ev) => setMessage(e.key, { priority: ev.target.value ? Number(ev.target.value) : null })}>
                      {PRIORITIES.map((p) => (
                        <option key={p.value} value={p.value}>
                          {p.label}
                        </option>
                      ))}
                    </select>
                  </label>
                )}
                {kind === 'telegram' && (
                  <label className="flex items-center gap-2 text-xs">
                    <input type="checkbox" className="size-4 accent-accent" disabled={disabled} checked={Boolean(m.silent)} onChange={(ev) => setMessage(e.key, { silent: ev.target.checked })} />
                    {t('Sans son', 'Without sound')}
                  </label>
                )}
                <span className="text-xs text-muted">
                  {t('Variables : ', 'Variables: ')}
                  {help.vars.map((v) => (
                    <code key={v} className="mr-1">{`{${v}}`}</code>
                  ))}
                </span>
              </div>
            </fieldset>
          )
        })}
      </div>
    </details>
  )
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
  const kind = kindOf(hook)
  const problem = hookProblem(hook)
  const info = KINDS.find((k) => k.key === kind)
  return (
    <div className="space-y-3 rounded-md border border-line p-4">
      <div className="flex flex-wrap items-end gap-3">
        <label className="w-44 text-sm">
          {t('Type', 'Type')}
          <select className={inputCls} value={kind} disabled={disabled} onChange={(e) => onChange({ ...hook, kind: e.target.value as WebhookKind })}>
            {KINDS.map((k) => (
              <option key={k.key} value={k.key}>
                {k.label}
              </option>
            ))}
          </select>
        </label>
        <label className="w-48 text-sm">
          {t('Nom', 'Name')}
          <input className={inputCls} value={hook.name} disabled={disabled} placeholder={kind === 'json' ? 'n8n' : info?.label} onChange={(e) => set('name', e.target.value)} />
        </label>
        <Switch checked={hook.enabled} onChange={(v) => set('enabled', v)} label={t('Actif', 'Active')} disabledReason={disabled ? t('Lecture seule', 'Read only') : null} />
      </div>
      {info && <p className="text-xs text-muted">{info.help}</p>}
      <div className="flex flex-wrap items-end gap-3">
        {kind === 'telegram' ? (
          <>
            <label className="min-w-72 flex-1 text-sm">
              {t('Jeton du bot', 'Bot token')}
              <input className={inputCls} type="password" autoComplete="off" value={hook.token ?? ''} disabled={disabled} placeholder="123456789:AAH…" onChange={(e) => set('token', e.target.value)} />
            </label>
            <label className="w-56 text-sm">
              {t('Identifiant de la conversation', 'Chat identifier')}
              <input className={inputCls} value={hook.chat_id ?? ''} disabled={disabled} placeholder="123456789" onChange={(e) => set('chat_id', e.target.value)} />
            </label>
          </>
        ) : (
          <label className="min-w-72 flex-1 text-sm">
            {kind === 'ntfy' ? t('Adresse du sujet', 'Topic address') : kind === 'discord' ? t('Adresse du webhook Discord', 'Discord webhook address') : t('Adresse appelée (POST)', 'Address called (POST)')}
            <input
              className={inputCls}
              value={hook.url}
              disabled={disabled}
              placeholder={kind === 'ntfy' ? 'https://ntfy.sh/maison-a7k2p9' : kind === 'discord' ? 'https://discord.com/api/webhooks/…' : 'https://n8n.maison/webhook/…'}
              onChange={(e) => set('url', e.target.value)}
            />
            {kind === 'ntfy' && <span className="mt-1 block text-xs text-muted">{t('Sur ntfy.sh, le nom du sujet sert de mot de passe : choisissez-en un difficile à deviner.', 'On ntfy.sh, the topic name works as a password: pick one hard to guess.')}</span>}
          </label>
        )}
        {kind === 'ntfy' && (
          <label className="w-64 text-sm">
            {t('Jeton d’accès (facultatif)', 'Access token (optional)')}
            <input className={inputCls} type="password" autoComplete="off" value={hook.token ?? ''} disabled={disabled} placeholder="tk_…" onChange={(e) => set('token', e.target.value || null)} />
          </label>
        )}
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
              <span className="block text-xs text-muted">{kind === 'json' ? t('JPEG en base64 dans le JSON', 'JPEG in base64 inside the JSON') : t('JPEG joint au message', 'JPEG attached to the message')}</span>
            </span>
          </label>
          {kind !== 'json' && (
            <label className="flex items-start gap-2">
              <input type="checkbox" className="mt-0.5 size-4 accent-accent" disabled={disabled} checked={hook.clip ?? false} onChange={(e) => set('clip', e.target.checked)} />
              <span>
                {t('Envoyer aussi une courte vidéo', 'Also send a short video')}
                <span className="block text-xs text-muted">{t('7 à 15 s à partir de 2 s avant l’événement, en second message, environ une minute après (le temps que l’enregistrement soit fini)', '7 to 15 s from 2 s before the event, as a second message, about a minute later (once the recording is finished)')}</span>
              </span>
            </label>
          )}
        </div>
      </div>
      <label className="flex flex-wrap items-center gap-2 text-sm">
        {t('Au plus une notification toutes les', 'At most one notification every')}
        <input
          type="number"
          min={0}
          max={COOLDOWN_MAX}
          className="h-8 w-20 rounded-md border border-line bg-bg-2 px-2 text-sm text-fg outline-none focus:border-accent disabled:opacity-60"
          value={hook.cooldown_minutes ?? 0}
          disabled={disabled}
          onChange={(e) => set('cooldown_minutes', e.target.value === '' ? 0 : Number(e.target.value))}
        />
        {t('min par caméra', 'min per camera')}
        <span className="block w-full text-xs text-muted">
          {t(
            '0 : sans limite. Le premier événement d’une caméra est envoyé en entier (mouvement, objets, fin), les suivants sont sautés pendant ce délai ; les passages de ligne sont comptés à part. Les pannes (stockage, caméra, enregistrement) passent toujours.',
            '0: no limit. The first event of a camera is sent whole (motion, objects, end), the next ones are skipped during the wait; line crossings are counted apart. Failures (storage, camera, recording) always go through.',
          )}
        </span>
      </label>
      <div className="space-y-1">
        <p className="text-sm">{t('Programmation', 'Schedule')}</p>
        <p className="text-xs text-muted">
          {t(
            'Jours et heures où les détections sont envoyées (mouvement, objets, passages), selon l’heure de début de l’événement. Les pannes (stockage, caméra, enregistrement) passent toujours.',
            'Days and hours when detections are sent (motion, objects, crossings), by the start time of the event. Failures (storage, camera, recording) always go through.',
          )}
        </p>
        <ScheduleEditor value={hook.schedule ?? []} onChange={(schedule) => set('schedule', schedule)} disabled={disabled} what={t('les détections sont envoyées', 'detections are sent')} />
      </div>
      {kind !== 'json' && <MessagesEditor hook={hook} kind={kind} onChange={onChange} disabled={disabled} />}
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
        <Button size="sm" icon={<Send className="size-3.5" />} disabledReason={disabled ? t('Lecture seule', 'Read only') : problem ?? (trying ? t('Appel en cours…', 'Calling…') : null)} onClick={() => void tryIt()}>
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
  const problem = hooks.some((h) => !h.name.trim()) ? t('Chaque webhook doit avoir un nom', 'Each webhook needs a name') : (hooks.map(hookProblem).find((p) => p !== null) ?? (hooks.some((h) => h.events.length === 0) ? t('Cochez au moins un type d’appel', 'Tick at least one kind of call') : null))
  const save = async () => {
    setBusy(true)
    setError(null)
    setDone(false)
    try {
      // Only the fields of its kind are kept (a token typed for another kind is not sent).
      const next = await setWebhooks(
        hooks.map((h) => {
          const kind = kindOf(h)
          const token = (h.token ?? '').trim()
          return {
            ...h,
            kind,
            name: h.name.trim(),
            url: kind === 'telegram' ? '' : h.url.trim(),
            token: (kind === 'ntfy' || kind === 'telegram') && token ? token : null,
            chat_id: kind === 'telegram' ? (h.chat_id ?? '').trim() : null,
            // RustNVR's JSON is never changed: no message for it.
            messages: kind === 'json' ? {} : cleanMessages(h),
            cooldown_minutes: h.cooldown_minutes ?? 0,
            schedule: h.schedule ?? [],
            clip: kind !== 'json' && (h.clip ?? false),
          }
        }),
      )
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
      {hooks.length === 0 && <p className="text-sm text-muted">{t('Aucun webhook. Ajoutez-en un : une adresse à appeler (n8n, Node-RED, Home Assistant, script…), un salon Discord, une notification ntfy ou un bot Telegram.', 'No webhook. Add one: an address to call (n8n, Node-RED, Home Assistant, script…), a Discord channel, a ntfy notification or a Telegram bot.')}</p>}
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
        <p className="mt-2">{t('Discord, ntfy et Telegram reçoivent à la place un message lisible : un titre (« Personne · Jardin »), le détail (objets, zones, durée…) et l’image jointe.', 'Discord, ntfy and Telegram get a readable message instead: a title (« Person · Garden »), the details (objects, zones, length…) and the attached picture.')}</p>
        {t('Un appel sans réponse en 10 s ou en erreur est retenté une fois après 5 s. Les adresses ne sont jamais écrites en entier dans le journal (seulement le nom de la machine).', 'A call without answer within 10 s or failing is tried once more after 5 s. Addresses are never written in full in the log (only the machine name).')}
      </details>
    </div>
  )
}
