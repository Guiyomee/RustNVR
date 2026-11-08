import { Check, Copy, Eye, EyeOff, PlugZap, RefreshCw } from 'lucide-react'
import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { describeError } from '../../api/client'
import { useReadOnly } from '../../auth/AuthGate'
import { createCamera, getDestinations, getSettings, getVideoDevices, probeCamera, probeStream, updateCamera, usbPreview } from '../../api/endpoints'
import { useAsync } from '../../hooks/useAsync'
import { useCapability, useData, useIsDemo } from '../../data/DataContext'
import { formatResolution } from '../../lib/format'
import type { Camera, CameraInput, ProbeResult, RecordMode, Transport } from '../../types/api'
import { Button } from '../ui/Button'
import { ConfirmDialog, Modal } from '../ui/Modal'
import { Segmented } from '../ui/Tabs'
import { t } from '../../lib/i18n'

interface FormState {
  name: string
  id: string
  /** Network camera (RTSP) or USB webcam. */
  source: 'rtsp' | 'usb'
  /** Webcam: device, format (empty: its default), size `WxH` (empty: its default), pictures per second, bit rate. */
  usbDevice: string
  usbFormat: string
  usbSize: string
  usbFps: string
  usbKbps: string
  url: string
  username: string
  password: string
  transport: Transport
  subUrl: string
  enabled: boolean
  record: boolean
  /** Where to record: `local` and/or destination ids. */
  destinations: string[]
  mode: RecordMode
  /** Record the sound with the video. */
  audio: boolean
  /** Per-camera retention; empty: none. */
  days: string
  gb: string
  /** Port of the ONVIF service (PTZ); empty: none. */
  onvif: string
}

const ID_RE = /^[a-z0-9][a-z0-9_-]{0,63}$/

export function slugify(name: string): string {
  return name
    .normalize('NFD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 64)
}

function validateUrl(url: string, label: string): string | null {
  if (!url.trim()) return t(`${label} est obligatoire.`, `${label} is required.`)
  let u: URL
  try {
    u = new URL(url.trim())
  } catch {
    return t(`${label} n'est pas une URL valide (ex. rtsp://192.168.1.20:554/stream1).`, `${label} is not a valid URL (e.g. rtsp://192.168.1.20:554/stream1).`)
  }
  if (u.protocol !== 'rtsp:') return t(`${label} doit commencer par rtsp:// (rtsps n'est pas pris en charge).`, `${label} must start with rtsp:// (rtsps is not supported).`)
  if (!u.hostname) return t(`${label} doit contenir l'adresse de la caméra.`, `${label} must contain the camera address.`)
  if (u.username || u.password) return t('Ne mettez pas les identifiants dans l’URL : utilisez les champs « Identifiant » et « Mot de passe ».', 'Do not put credentials in the URL: use the « Username » and « Password » fields.')
  return null
}

export function validate(f: FormState, existingIds: string[], editing: boolean): Partial<Record<keyof FormState, string>> {
  const e: Partial<Record<keyof FormState, string>> = {}
  if (!f.name.trim()) e.name = t('Le nom est obligatoire.', 'The name is required.')
  else if (f.name.length > 128) e.name = t('128 caractères au maximum.', '128 characters at most.')
  if (!editing) {
    if (!ID_RE.test(f.id)) e.id = t('Lettres minuscules, chiffres, « - » et « _ », 64 caractères au maximum, en commençant par une lettre ou un chiffre.', 'Lowercase letters, digits, « - » and « _ », 64 characters at most, starting with a letter or a digit.')
    else if (existingIds.includes(f.id)) e.id = t('Cet identifiant est déjà utilisé.', 'This identifier is already used.')
    else if (f.id === 'probe') e.id = t('Cet identifiant est réservé.', 'This identifier is reserved.')
  }
  const whole = (v: string, min: number, max: number) => !v.trim() || (Number.isInteger(Number(v)) && Number(v) >= min && Number(v) <= max)
  if (f.source === 'usb') {
    if (!f.usbDevice.trim().startsWith('/dev/')) e.usbDevice = t('Choisissez la webcam (chemin sous /dev, par exemple /dev/video0).', 'Choose the webcam (path under /dev, e.g. /dev/video0).')
    if (f.usbSize && !/^\d+x\d+$/.test(f.usbSize)) e.usbSize = t('Taille au format 1280x720, ou vide.', 'Size as 1280x720, or empty.')
    if (!whole(f.usbFps, 1, 120)) e.usbFps = t('De 1 à 120 images par seconde, ou vide.', '1 to 120 pictures per second, or empty.')
    if (!f.usbKbps.trim() || !whole(f.usbKbps, 100, 50000)) e.usbKbps = t('Débit de 100 à 50 000 kbit/s.', 'Bit rate from 100 to 50,000 kbit/s.')
  } else {
    const u = validateUrl(f.url, t('L’URL du flux principal', 'The main stream URL'))
    if (u) e.url = u
    if (f.subUrl.trim()) {
      const s = validateUrl(f.subUrl, t('L’URL du sous-flux', 'The sub stream URL'))
      if (s) e.subUrl = s
    }
    if (f.password && !f.username.trim()) e.username = t("Un identifiant est requis lorsqu'un mot de passe est saisi.", "A username is required when a password is entered.")
  }
  if (f.record && f.destinations.length === 0) e.destinations = t("Choisissez au moins un lieu d'enregistrement.", "Choose at least one recording place.")
  const positive = (v: string) => !v.trim() || (Number.isInteger(Number(v)) && Number(v) > 0)
  if (!positive(f.days) || Number(f.days) > 3650) e.days = t('Nombre de jours entre 1 et 3650, ou vide.', 'Number of days between 1 and 3650, or empty.')
  if (!positive(f.gb)) e.gb = t('Nombre entier de Go, ou vide.', 'Whole number of GB, or empty.')
  if (f.source === 'rtsp' && !whole(f.onvif, 1, 65535)) e.onvif = t('Port entre 1 et 65535, ou vide.', 'Port between 1 and 65535, or empty.')
  return e
}

/** The webcam settings of the form. */
function usbOf(f: FormState): NonNullable<CameraInput['usb']> {
  const [w, h] = f.usbSize ? f.usbSize.split('x').map(Number) : [null, null]
  return {
    device: f.usbDevice.trim(),
    input_format: f.usbFormat || null,
    width: w ?? null,
    height: h ?? null,
    framerate: f.usbFps.trim() ? Number(f.usbFps) : null,
    bitrate_kbps: Number(f.usbKbps),
  }
}

/** TOML basic strings accept JSON escapes. */
const str = (s: string) => JSON.stringify(s)

export function envVarName(id: string): string {
  return `RUSTNVR_${id.toUpperCase().replace(/[^A-Z0-9]/g, '_')}_PASSWORD`
}

export function toToml(f: FormState): string {
  const env = envVarName(f.id)
  const stream = (url: string) => {
    const parts = [`url = ${str(url.trim())}`]
    if (f.username.trim()) parts.push(`username = ${str(f.username.trim())}`)
    if (f.password) parts.push(`password_env = ${str(env)}`)
    if (f.transport !== 'tcp') parts.push(`transport = ${str(f.transport)}`)
    return `{ ${parts.join(', ')} }`
  }
  const lines = ['[[cameras]]', `id = ${str(f.id)}`, `name = ${str(f.name.trim())}`]
  if (!f.enabled) lines.push('enabled = false')
  if (f.source === 'usb') {
    const u = usbOf(f)
    const parts = [`device = ${str(u.device)}`]
    if (u.input_format) parts.push(`input_format = ${str(u.input_format)}`)
    if (u.width && u.height) parts.push(`width = ${u.width}, height = ${u.height}`)
    if (u.framerate) parts.push(`framerate = ${u.framerate}`)
    if (u.bitrate_kbps !== 2000) parts.push(`bitrate_kbps = ${u.bitrate_kbps}`)
    lines.push(`usb = { ${parts.join(', ')} }`)
  } else {
    lines.push(`main = ${stream(f.url)}`)
    if (f.subUrl.trim()) lines.push(`sub = ${stream(f.subUrl)}`)
  }
  const keep = [f.days.trim() && `, retention_days = ${f.days.trim()}`, f.gb.trim() && `, max_size_gb = ${f.gb.trim()}`, !f.audio && ", audio = false"].filter(Boolean).join('')
  lines.push(`record = { enabled = ${f.record}, mode = "${f.mode}", destinations = [${f.destinations.map(str).join(', ')}]${keep} }`)
  if (f.source === 'rtsp' && f.onvif.trim()) lines.push(`onvif = { port = ${Number(f.onvif)} }`)
  return lines.join('\n')
}

function toInput(f: FormState, editing: boolean): CameraInput {
  const username = f.username.trim() || null
  // An empty field sends null: no password on creation, unchanged when editing.
  const password = f.password || null
  return {
    ...(editing ? {} : { id: f.id }),
    name: f.name.trim(),
    enabled: f.enabled,
    record: f.record,
    ...(f.source === 'usb'
      ? { main: null, sub: null, usb: usbOf(f), onvif_port: null }
      : {
          main: { url: f.url.trim(), username, password, transport: f.transport },
          sub: f.subUrl.trim() ? { url: f.subUrl.trim(), username, password, transport: f.transport } : null,
          usb: null,
          onvif_port: f.onvif.trim() ? Number(f.onvif) : null,
        }),
    record_destinations: f.destinations,
    record_mode: f.mode,
    retention_days: f.days.trim() ? Number(f.days) : null,
    max_size_gb: f.gb.trim() ? Number(f.gb) : null,
    record_audio: f.audio,
  }
}

function Field({ label, error, hint, children, htmlFor }: { label: string; error?: string; hint?: ReactNode; children: ReactNode; htmlFor: string }) {
  return (
    <div className="space-y-1">
      <label htmlFor={htmlFor} className="block text-xs font-medium text-muted">
        {label}
      </label>
      {children}
      {error ? (
        <p id={`${htmlFor}-err`} className="text-xs text-err">
          {error}
        </p>
      ) : (
        hint && <p className="text-xs text-muted">{hint}</p>
      )}
    </div>
  )
}

const inputCls = (err?: string) =>
  `h-9 w-full rounded-md border bg-bg-2 px-3 text-sm outline-none focus:border-accent disabled:opacity-60 ${err ? 'border-err' : 'border-line'}`

/** New cameras do not record unless asked to. */
const empty: FormState = {
  name: '',
  id: '',
  source: 'rtsp',
  usbDevice: '',
  usbFormat: '',
  usbSize: '',
  usbFps: '',
  usbKbps: '2000',
  url: '',
  username: '',
  password: '',
  transport: 'tcp',
  subUrl: '',
  enabled: true,
  record: false,
  destinations: ['local'],
  mode: 'continuous',
  audio: true,
  days: '',
  gb: '',
  onvif: '',
}

function fromCamera(c: Camera): FormState {
  const usb = c.edit?.usb
  return {
    ...empty,
    name: c.name,
    id: c.id,
    enabled: c.enabled,
    record: c.recording_enabled,
    ...(usb
      ? {
          source: 'usb' as const,
          usbDevice: usb.device,
          usbFormat: usb.input_format ?? '',
          usbSize: usb.width && usb.height ? `${usb.width}x${usb.height}` : '',
          usbFps: usb.framerate?.toString() ?? '',
          usbKbps: (usb.bitrate_kbps ?? 2000).toString(),
        }
      : {}),
    url: c.edit?.main_url ?? '',
    subUrl: c.edit?.sub_url ?? '',
    username: c.edit?.username ?? '',
    transport: c.edit?.transport ?? 'tcp',
    destinations: c.record_destinations.length > 0 ? c.record_destinations : ['local'],
    mode: c.record_mode,
    audio: c.record_audio,
    days: c.retention_days?.toString() ?? '',
    gb: c.max_size_gb?.toString() ?? '',
    onvif: c.edit?.onvif_port?.toString() ?? '',
  }
}

/** Recording destinations to choose from, with their state. */
function DestinationChoice({ value, onChange, error }: { value: string[]; onChange: (v: string[]) => void; error?: string }) {
  const demo = useIsDemo()
  const list = useAsync((signal) => (demo ? Promise.resolve(null) : getDestinations()).then((d) => (signal.aborted ? null : d)), [demo])
  const options = list.data ?? [{ id: 'local', name: t('Disque local', 'Local disk'), kind: 'local' as const, available: true, unavailable_reason: null, path: '' }]
  return (
    <fieldset className="space-y-1.5 rounded-md border border-line p-3">
      <legend className="px-1 text-xs font-medium text-muted">{t("Lieux d'enregistrement", "Recording places")}</legend>
      {options.map((d) => (
        <label key={d.id} className="flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            className="mt-0.5 size-4 accent-accent"
            checked={value.includes(d.id)}
            onChange={(e) => onChange(e.target.checked ? [...value, d.id] : value.filter((x) => x !== d.id))}
          />
          <span>
            {d.name} <span className="text-xs text-muted">{d.kind === 'local' ? t('(disque local)', '(local disk)') : `(${d.path})`}</span>
            {!d.available && <span className="block text-xs text-warn">{t('Indisponible : ', 'Unavailable: ')}{d.unavailable_reason}</span>}
          </span>
        </label>
      ))}
      {error ? (
        <p className="text-xs text-err">{error}</p>
      ) : (
        <p className="text-xs text-muted">
          {t('Chaque lieu reçoit sa propre copie. Une caméra enregistrée seulement sur un NAS enregistre sur le disque local tant que le NAS est indisponible. Les points de stockage se gèrent dans Paramètres → Stockage.', 'Each place gets its own copy. A camera recorded only on a NAS records on the local disk while the NAS is unavailable. Storage destinations are managed in Settings → Storage.')}
        </p>
      )}
    </fieldset>
  )
}

/** The webcam of a USB camera: chosen among the devices of the server, with what they can send. */
function UsbFields({ f, set, shown }: { f: FormState; set: <K extends keyof FormState>(k: K, v: FormState[K]) => void; shown: (k: keyof FormState) => string | undefined }) {
  const demo = useIsDemo()
  const list = useAsync(() => (demo ? Promise.resolve({ devices: [] }) : getVideoDevices()), [demo])
  const devices = (list.data?.devices ?? []).filter((d) => d.formats.length > 0 || d.error)
  const chosen = devices.find((d) => d.by_id === f.usbDevice || d.path === f.usbDevice)
  const format = chosen?.formats.find((x) => x.format === f.usbFormat)
  const sizes = (format ?? chosen?.formats[0])?.sizes.filter((s) => /^\d+x\d+$/.test(s)) ?? []
  const copy = f.usbFormat === 'h264'
  return (
    <>
      <div className="sm:col-span-2">
        <Field
          label={t('Webcam', 'Webcam')}
          htmlFor="cam-usb"
          error={shown('usbDevice')}
          hint={
            list.loading
              ? t('Recherche des webcams du serveur… (FFmpeg lit leurs formats)', 'Looking for the webcams of the server… (FFmpeg reads their formats)')
              : list.error
                ? t(`Liste indisponible : ${describeError(list.error)}. Saisissez le chemin.`, `List unavailable: ${describeError(list.error)}. Type the path.`)
                : devices.length === 0
                  ? t('Aucune webcam vue par le serveur. Sous Docker, ajoutez le périphérique (devices: /dev/video0) ; dans un LXC, passez-le au conteneur. Le chemin peut aussi être saisi.', 'No webcam seen by the server. Under Docker, add the device (devices: /dev/video0); in an LXC, pass it to the container. The path can also be typed.')
                  : t('Le chemin /dev/v4l/by-id ne change pas si la webcam est branchée sur un autre port.', 'The /dev/v4l/by-id path does not change if the webcam is plugged into another port.')
          }
        >
          <div className="flex gap-2">
            {devices.length > 0 ? (
              <select
                id="cam-usb"
                className={inputCls(shown('usbDevice'))}
                value={f.usbDevice}
                onChange={(e) => {
                  set('usbDevice', e.target.value)
                  set('usbFormat', '')
                  set('usbSize', '')
                }}
              >
                <option value="">{t('— choisir —', '— choose —')}</option>
                {devices.map((d) => (
                  <option key={d.path} value={d.by_id ?? d.path}>
                    {(d.name ?? d.path) + ` (${d.path})` + (d.error ? t(` — ${d.error}`, ` — ${d.error}`) : '')}
                  </option>
                ))}
              </select>
            ) : (
              <input id="cam-usb" className={inputCls(shown('usbDevice'))} value={f.usbDevice} onChange={(e) => set('usbDevice', e.target.value)} placeholder="/dev/video0" autoComplete="off" spellCheck={false} />
            )}
            <Button icon={<RefreshCw className="size-4" />} disabledReason={demo ? t('Indisponible en mode démo.', 'Not available in demo mode.') : list.loading ? t('Recherche en cours…', 'Searching…') : null} onClick={list.reload}>
              {t('Relire', 'Reload')}
            </Button>
          </div>
        </Field>
      </div>
      <Field
        label={t('Format demandé à la webcam', 'Format asked to the webcam')}
        htmlFor="cam-usb-format"
        hint={copy ? t('H.264 de la webcam enregistré tel quel : rien n’est encodé, presque aucun processeur.', 'The webcam’s H.264 recorded as is: nothing is encoded, almost no processor.') : t('MJPEG : images nettes jusqu’en 1080p. RustNVR encode en H.264, sur la carte graphique si elle sert aux conversions.', 'MJPEG: sharp pictures up to 1080p. RustNVR encodes to H.264, on the graphics card if it is used for conversions.')}
      >
        <select id="cam-usb-format" className={inputCls()} value={f.usbFormat} onChange={(e) => (set('usbFormat', e.target.value), set('usbSize', ''))}>
          <option value="">{t('Celui de la webcam (par défaut)', 'The webcam’s own (default)')}</option>
          {(chosen?.formats ?? []).map((x) => (
            <option key={x.format} value={x.format}>
              {x.format === 'h264' ? t(`${x.description} — copié sans encodage`, `${x.description} — copied without encoding`) : x.description}
            </option>
          ))}
          {!chosen && f.usbFormat && <option value={f.usbFormat}>{f.usbFormat}</option>}
        </select>
      </Field>
      <Field label={t('Taille de l’image', 'Picture size')} htmlFor="cam-usb-size" error={shown('usbSize')}>
        {sizes.length > 0 ? (
          <select id="cam-usb-size" className={inputCls(shown('usbSize'))} value={f.usbSize} onChange={(e) => set('usbSize', e.target.value)}>
            <option value="">{t('Celle de la webcam (par défaut)', 'The webcam’s own (default)')}</option>
            {sizes.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
        ) : (
          <input id="cam-usb-size" className={inputCls(shown('usbSize'))} value={f.usbSize} onChange={(e) => set('usbSize', e.target.value)} placeholder="1280x720" autoComplete="off" />
        )}
      </Field>
      <Field label={t('Images par seconde', 'Pictures per second')} htmlFor="cam-usb-fps" error={shown('usbFps')} hint={t('Vide : celles de la webcam. 10 à 15 suffisent souvent et économisent le processeur.', 'Empty: the webcam’s. 10 to 15 are often enough and save processor.')}>
        <input id="cam-usb-fps" className={inputCls(shown('usbFps'))} inputMode="numeric" value={f.usbFps} onChange={(e) => set('usbFps', e.target.value)} placeholder="15" />
      </Field>
      {!copy && (
        <Field label={t('Débit maximal (kbit/s)', 'Maximum bit rate (kbit/s)')} htmlFor="cam-usb-kbps" error={shown('usbKbps')} hint={t('2000 conviennent au 720p ; 4000 au 1080p.', '2000 suit 720p; 4000 suits 1080p.')}>
          <input id="cam-usb-kbps" className={inputCls(shown('usbKbps'))} inputMode="numeric" value={f.usbKbps} onChange={(e) => set('usbKbps', e.target.value)} />
        </Field>
      )}
    </>
  )
}

type ProbeState = { kind: 'idle' } | { kind: 'running' } | { kind: 'ok'; result: ProbeResult } | { kind: 'usb'; preview: string } | { kind: 'error'; message: string }

function ProbeOutcome({ state }: { state: ProbeState }) {
  if (state.kind === 'idle') return null
  if (state.kind === 'running') return <p className="text-sm text-muted">{t("Connexion à la caméra… (jusqu'à 30 s)", "Connecting to the camera… (up to 30 s)")}</p>
  if (state.kind === 'error') return <p className="rounded-md border border-err/40 bg-err/10 px-3 py-2 text-sm text-err">{t('Échec : ', 'Failed: ')}{state.message}</p>
  if (state.kind === 'usb')
    return (
      <div className="rounded-md border border-line px-3 py-2 text-sm">
        <p className="text-muted">{t('Image de la webcam avec ces réglages (une image noire ou figée : un autre format ou une autre taille).', 'Picture of the webcam with these settings (a black or still picture: another format or size).')}</p>
        <LivePreview src={state.preview} />
      </div>
    )
  const { result } = state
  const v = result.video
  return (
    <div className="rounded-md border border-ok/40 bg-ok/10 px-3 py-2 text-sm">
      {v ? (
        <p className="text-ok">
          {t('Connexion réussie : ', 'Connected: ')}{v.codec.toUpperCase()} {formatResolution(v.width, v.height)}{t(', première image après ', ', first picture after ')}{v.time_to_key_frame_ms}{t(' ms.', ' ms.')}
        </p>
      ) : (
        <p className="text-warn">{t("La caméra répond mais n'annonce aucun flux H.264/H.265 enregistrable.", "The camera answers but announces no H.264/H.265 stream that can be recorded.")}</p>
      )}
      {v && <AudioNote streams={result.streams} />}
      <p className="mt-1 text-xs text-muted">{t('Flux annoncés : ', 'Announced streams: ')}{result.streams.map((s) => `${s.media}/${s.encoding}`).join(', ') || 'aucun'}</p>
      {result.preview && <LivePreview src={result.preview} />}
    </div>
  )
}

/** What happens to the camera's sound: AAC is recorded as is, G.711 converted to Opus. */
function AudioNote({ streams }: { streams: ProbeResult['streams'] }) {
  const audio = streams.filter((s) => s.media === 'audio')
  const kinds = audio.map((s) => s.encoding.toLowerCase())
  if (audio.length === 0) return <p className="mt-1 text-xs text-muted">{t('Pas de son annoncé : vidéo seule.', 'No sound announced: video only.')}</p>
  if (kinds.includes('mpeg4-generic')) return <p className="mt-1 text-xs text-muted">{t('Son AAC : enregistré avec la vidéo.', 'AAC sound: recorded with the video.')}</p>
  if (kinds.includes('pcmu') || kinds.includes('pcma')) {
    const name = kinds.includes('pcmu') ? 'G.711 µ-law (PCMU)' : 'G.711 A-law (PCMA)'
    return <p className="mt-1 text-xs text-muted">{t(`Son ${name} : converti en Opus et enregistré avec la vidéo.`, `${name} sound: converted to Opus and recorded with the video.`)}</p>
  }
  const name = audio.map((s) => s.encoding.toUpperCase()).join(', ')
  return (
    <p className="mt-1 text-xs text-warn">
      {t(`Son ${name} : pas enregistré (RustNVR enregistre le son AAC et G.711). La vidéo est enregistrée normalement ; pour le son, choisissez AAC ou G.711 dans les réglages audio de la caméra.`, `${name} sound: not recorded (RustNVR records AAC and G.711 sound). The video is recorded normally; for the sound, choose AAC or G.711 in the audio settings of the camera.`)}
    </p>
  )
}

/** The camera live for a minute (motion JPEG), right after a successful test. */
function LivePreview({ src }: { src: string }) {
  const [state, setState] = useState<'loading' | 'playing' | 'ended'>('loading')
  return (
    <figure className="mt-2">
      <div className="relative aspect-video overflow-hidden rounded-md bg-black">
        <img
          src={src}
          alt={t('Aperçu en direct de la caméra', 'Live preview of the camera')}
          className="size-full object-contain"
          onLoad={() => setState('playing')}
          onError={() => setState('ended')}
        />
        {state === 'loading' && <span className="absolute inset-0 grid place-items-center text-xs text-muted">{t('Ouverture du direct…', 'Opening the live view…')}</span>}
      </div>
      <figcaption className="mt-1 text-xs text-muted">
        {state === 'ended'
          ? t('Aperçu terminé : refaites le test pour le revoir.', 'Preview over: test again to see it again.')
          : t('Aperçu en direct, 4 images par seconde pendant 1 minute (refaites le test pour le relancer).', 'Live preview, 4 pictures per second for 1 minute (test again to restart it).')}
      </figcaption>
    </figure>
  )
}

/** Values found on the network (Cameras → « Rechercher sur le réseau ») for a new camera. */
export interface CameraPreset {
  name: string
  url: string
  subUrl: string
  username: string
  password: string
  /** ONVIF port that answered, empty when unknown. */
  onvif?: string
}

/** `base`, or `base-2`, `base-3`… when taken. */
function freeId(base: string, existingIds: string[]): string {
  const id = base || 'camera'
  if (!existingIds.includes(id)) return id
  for (let n = 2; ; n++) {
    const next = `${id.slice(0, 60)}-${n}`
    if (!existingIds.includes(next)) return next
  }
}

export function CameraForm({ open, onClose, camera, existingIds, preset = null }: { open: boolean; onClose: () => void; camera: Camera | null; existingIds: string[]; preset?: CameraPreset | null }) {
  const canManage = useCapability('camera_management') === true
  const usbCameras = useCapability('usb_cameras') === true
  const viewerReason = useReadOnly()
  const demo = useIsDemo()
  const { source } = useData()
  const editing = camera !== null
  // Cameras of config.toml can only be changed in that file.
  const readOnly = editing && !camera.managed
  const [f, setF] = useState<FormState>(empty)
  // The form as opened: closing asks first once something was changed.
  const [initial, setInitial] = useState<FormState>(empty)
  const [confirmDiscard, setConfirmDiscard] = useState(false)
  const [idTouched, setIdTouched] = useState(false)
  const [submitted, setSubmitted] = useState(false)
  const [showPassword, setShowPassword] = useState(false)
  const [toml, setToml] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const [probe, setProbe] = useState<ProbeState>({ kind: 'idle' })
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)

  useEffect(() => {
    if (!open) return
    const start = camera ? fromCamera(camera) : preset ? { ...empty, ...preset, onvif: preset.onvif ?? '', id: freeId(slugify(preset.name), existingIds) } : empty
    setF(start)
    setInitial(start)
    setConfirmDiscard(false)
    setIdTouched(editing)
    setSubmitted(false)
    setShowPassword(false)
    setToml(null)
    setCopied(false)
    setProbe({ kind: 'idle' })
    setSaving(false)
    setSaveError(null)
    // existingIds is read when opening only: a new list must not reset what is typed.
  }, [open, camera, editing, preset])

  // New camera: start from the default destination (Settings → Stockage).
  const canReadSettings = useCapability('settings_write') === true
  useEffect(() => {
    if (!open || editing || demo || !canReadSettings) return
    let alive = true
    Promise.all([getSettings(), getDestinations()]).then(
      ([s, list]) => {
        // A removed default falls back to the local disk, as on the server.
        const d = list.some((x) => x.id === s.default_destination) ? s.default_destination : 'local'
        const withDefault = (prev: FormState) => (prev.destinations.length === 1 && prev.destinations[0] === 'local' ? { ...prev, destinations: [d] } : prev)
        if (alive) {
          setF(withDefault)
          setInitial(withDefault)
        }
      },
      () => undefined,
    )
    return () => {
      alive = false
    }
  }, [open, editing, demo, canReadSettings])

  const errors = useMemo(() => validate(f, existingIds, editing), [f, existingIds, editing])
  const valid = Object.keys(errors).length === 0
  const shown = (k: keyof FormState) => (submitted ? errors[k] : undefined)
  const set = <K extends keyof FormState>(k: K, v: FormState[K]) => {
    setF((prev) => {
      const next = { ...prev, [k]: v }
      if (k === 'name' && !idTouched) next.id = slugify(String(v))
      return next
    })
    setToml(null)
    setSaveError(null)
  }

  const close = () => {
    // Do not keep the password in memory once the dialog is closed.
    setF(empty)
    setInitial(empty)
    setConfirmDiscard(false)
    onClose()
  }
  const dirty = JSON.stringify(f) !== JSON.stringify(initial)
  // Escape, the cross and Annuler: what was typed is not lost by mistake.
  const requestClose = () => (dirty && !saving ? setConfirmDiscard(true) : close())

  const runProbe = async () => {
    setSubmitted(true)
    if (f.source === 'usb') {
      if (errors.usbDevice || errors.usbSize || errors.usbFps) return
      setProbe({ kind: 'running' })
      try {
        const { preview } = await usbPreview(usbOf(f))
        setProbe({ kind: 'usb', preview })
      } catch (e) {
        setProbe({ kind: 'error', message: describeError(e) })
      }
      return
    }
    if (errors.url) return
    setProbe({ kind: 'running' })
    try {
      // When editing without retyping the password, test the stored settings.
      const result =
        editing && camera?.managed && !f.password
          ? await probeCamera(camera.id)
          : await probeStream({ url: f.url.trim(), username: f.username.trim() || null, password: f.password || null, transport: f.transport })
      setProbe({ kind: 'ok', result })
    } catch (e) {
      setProbe({ kind: 'error', message: describeError(e) })
    }
  }

  const save = async () => {
    setSubmitted(true)
    if (!valid) return
    setSaving(true)
    setSaveError(null)
    try {
      const saved = editing ? await updateCamera(camera.id, toInput(f, true)) : await createCamera(toInput(f, false))
      source.store.upsertCamera(saved)
      source.store.notify('info', editing ? t(`${saved.name} : modifiée`, `${saved.name}: changed`) : t(`${saved.name} : ajoutée`, `${saved.name}: added`), saved.id)
      close()
    } catch (e) {
      setSaveError(describeError(e))
    } finally {
      setSaving(false)
    }
  }

  const saveReason = demo
    ? t('Mode démo : rien n’est enregistré.', 'Demo mode: nothing is saved.')
    : viewerReason
      ? viewerReason
    : !canManage
    ? t("Ce serveur ne permet pas de gérer les caméras depuis l'interface : copiez la configuration générée dans config.toml.", "This server does not allow managing cameras from the interface: copy the generated configuration into config.toml.")
    : readOnly
      ? t('Caméra déclarée dans config.toml : modifiez ce fichier (le bouton « Générer la configuration » prépare le bloc), puis redémarrez RustNVR.', 'Camera declared in config.toml: edit this file (the « Generate the configuration » button prepares the block), then restart RustNVR.')
      : null

  return (
    <>
    <Modal
      open={open}
      guarded
      wide
      title={editing ? t(`Modifier ${camera.name}`, `Edit ${camera.name}`) : t('Ajouter une caméra', 'Add a camera')}
      onClose={requestClose}
      footer={
        <>
          <Button onClick={requestClose}>{t('Annuler', 'Cancel')}</Button>
          <Button
            onClick={() => {
              setSubmitted(true)
              if (valid) setToml(toToml(f))
            }}
          >
            {t('Générer la configuration', 'Generate the configuration')}
          </Button>
          <Button variant="primary" disabledReason={saveReason ?? (saving ? t('Enregistrement en cours…', 'Saving…') : null)} onClick={() => void save()}>
            {saving ? t('Enregistrement…', 'Saving…') : t('Enregistrer', 'Save')}
          </Button>
        </>
      }
    >
      {readOnly && (
        <p className="mb-4 rounded-md border border-warn/40 bg-warn/10 px-3 py-2 text-xs text-warn">
          {t("Cette caméra est déclarée dans config.toml : elle n'est pas modifiable depuis l'interface.", "This camera is declared in config.toml: it cannot be changed from the interface.")}
        </p>
      )}
      <form className="grid gap-4 sm:grid-cols-2" onSubmit={(e) => (e.preventDefault(), void save())} noValidate>
        <Field label={t('Nom', 'Name')} htmlFor="cam-name" error={shown('name')}>
          <input id="cam-name" className={inputCls(shown('name'))} value={f.name} onChange={(e) => set('name', e.target.value)} placeholder={t("Porte d'entrée", "Front door")} autoComplete="off" />
        </Field>
        <Field label={t('Identifiant technique', 'Technical identifier')} htmlFor="cam-id" error={shown('id')} hint={t('Utilisé dans les noms de dossiers ; ne pourra plus changer.', 'Used in folder names; cannot be changed later.')}>
          <input
            id="cam-id"
            className={inputCls(shown('id'))}
            value={f.id}
            disabled={editing}
            onChange={(e) => {
              setIdTouched(true)
              set('id', e.target.value)
            }}
            placeholder="porte-entree"
            autoComplete="off"
          />
        </Field>
        {(usbCameras || f.source === 'usb') && (
          <div className="flex flex-wrap items-center gap-3 text-sm sm:col-span-2">
            <span className="text-xs font-medium text-muted">{t('Source', 'Source')}</span>
            <Segmented
              label={t('Source de la caméra', 'Camera source')}
              options={[
                { value: 'rtsp', label: t('Caméra réseau (RTSP)', 'Network camera (RTSP)') },
                { value: 'usb', label: t('Webcam USB', 'USB webcam') },
              ]}
              value={f.source}
              onChange={(v) => set('source', v)}
            />
          </div>
        )}
        {f.source === 'usb' ? (
          <UsbFields f={f} set={set} shown={shown} />
        ) : (
          <>
        <div className="sm:col-span-2">
          <Field
            label={t('URL RTSP du flux principal', 'RTSP URL of the main stream')}
            htmlFor="cam-url"
            error={shown('url')}
            hint={readOnly ? t("L'API ne renvoie pas l'URL des caméras de config.toml.", "The API does not return the URL of config.toml cameras.") : t('Exemples : Hikvision /Streaming/Channels/101, Dahua /cam/realmonitor?channel=1&subtype=0, Reolink /h264Preview_01_main.', 'Examples: Hikvision /Streaming/Channels/101, Dahua /cam/realmonitor?channel=1&subtype=0, Reolink /h264Preview_01_main.')}
          >
            <input id="cam-url" className={inputCls(shown('url'))} value={f.url} onChange={(e) => set('url', e.target.value)} placeholder="rtsp://192.168.1.20:554/Streaming/Channels/101" autoComplete="off" spellCheck={false} />
          </Field>
        </div>
        <Field label={t('Identifiant (facultatif)', 'Username (optional)')} htmlFor="cam-user" error={shown('username')}>
          <input id="cam-user" className={inputCls(shown('username'))} value={f.username} onChange={(e) => set('username', e.target.value)} autoComplete="off" />
        </Field>
        <Field
          label={t('Mot de passe', 'Password')}
          htmlFor="cam-pass"
          hint={editing && camera?.edit?.has_password ? t('Laissez vide pour conserver le mot de passe enregistré.', 'Leave empty to keep the saved password.') : t('Jamais réaffiché ni renvoyé par le serveur.', 'Never shown again nor returned by the server.')}
        >
          <div className="relative">
            <input
              id="cam-pass"
              type={showPassword ? 'text' : 'password'}
              className={`${inputCls()} pr-10`}
              value={f.password}
              placeholder={editing && camera?.edit?.has_password ? t('•••••••• (inchangé)', '•••••••• (unchanged)') : ''}
              onChange={(e) => set('password', e.target.value)}
              autoComplete="new-password"
            />
            <button type="button" className="absolute top-1/2 right-2 -translate-y-1/2 text-muted hover:text-fg" aria-label={showPassword ? t('Masquer le mot de passe', 'Hide the password') : t('Afficher le mot de passe pendant la saisie', 'Show the password while typing')} onClick={() => setShowPassword(!showPassword)}>
              {showPassword ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
            </button>
          </div>
        </Field>
        <Field label={t('Transport', 'Transport')} htmlFor="cam-transport" hint={t('TCP est recommandé ; UDP est expérimental (réseau local uniquement).', 'TCP is recommended; UDP is experimental (local network only).')}>
          <select id="cam-transport" className={inputCls()} value={f.transport} onChange={(e) => set('transport', e.target.value as Transport)}>
            <option value="tcp">{t('RTSP sur TCP', 'RTSP over TCP')}</option>
            <option value="udp">{t('RTP sur UDP (expérimental)', 'RTP over UDP (experimental)')}</option>
          </select>
        </Field>
        <Field label={t('URL du sous-flux (facultatif)', 'Sub stream URL (optional)')} htmlFor="cam-sub" error={shown('subUrl')} hint={t('Basse résolution, utilisé pour le direct ; mêmes identifiants que le flux principal.', 'Low resolution, used for the live view; same credentials as the main stream.')}>
          <input id="cam-sub" className={inputCls(shown('subUrl'))} value={f.subUrl} onChange={(e) => set('subUrl', e.target.value)} placeholder="rtsp://192.168.1.20:554/Streaming/Channels/102" autoComplete="off" spellCheck={false} />
        </Field>
        <Field
          label={t('Port ONVIF, pour les caméras motorisées (facultatif)', 'ONVIF port, for motorised cameras (optional)')}
          htmlFor="cam-onvif"
          error={shown('onvif')}
          hint={t('Active le pilotage PTZ (flèches, zoom, positions) dans le direct, avec les identifiants du flux principal. Souvent 80 ; Reolink 8000, Tapo 2020. Vide : pas de PTZ.', 'Enables PTZ control (arrows, zoom, positions) in the live view, with the credentials of the main stream. Often 80; Reolink 8000, Tapo 2020. Empty: no PTZ.')}
        >
          <input id="cam-onvif" className={inputCls(shown('onvif'))} inputMode="numeric" value={f.onvif} onChange={(e) => set('onvif', e.target.value)} placeholder={t('aucun', 'none')} autoComplete="off" />
        </Field>
          </>
        )}
        <div className="flex flex-col gap-2 sm:col-span-2">
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" className="size-4 accent-accent" checked={f.enabled} onChange={(e) => set('enabled', e.target.checked)} />
            {t('Caméra activée (connexion et direct)', 'Camera on (connection and live view)')}
          </label>
          <label className="flex items-center gap-2 text-sm">
            <input type="checkbox" className="size-4 accent-accent" checked={f.record} onChange={(e) => set('record', e.target.checked)} />
            {t('Enregistrer les vidéos', 'Record videos')}
          </label>
          {f.record && (
            <fieldset className="ml-6 space-y-1 text-sm">
              <legend className="sr-only">{t('Quand enregistrer', 'When to record')}</legend>
              <label className="flex items-start gap-2">
                <input type="radio" name="cam-mode" className="mt-0.5 accent-accent" checked={f.mode === 'continuous'} onChange={() => set('mode', 'continuous')} />
                <span>{t('En continu', 'Continuously')}<span className="block text-xs text-muted">{t('Tout le temps, sans interruption.', 'All the time, without interruption.')}</span></span>
              </label>
              <label className="flex items-start gap-2">
                <input type="radio" name="cam-mode" className="mt-0.5 accent-accent" checked={f.mode === 'motion'} onChange={() => set('mode', 'motion')} />
                <span>
                  {t('Seulement sur mouvement', 'Only on motion')}
                  <span className="block text-xs text-muted">{t("Quelques secondes avant et après chaque mouvement. Activez la détection dans l'onglet « Détection » de la caméra.", "A few seconds before and after each motion. Turn detection on in the « Detection » tab of the camera.")}</span>
                </span>
              </label>
            </fieldset>
          )}
          {f.record && f.source === 'rtsp' && (
            <label className="ml-6 flex items-start gap-2 text-sm">
              <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={f.audio} onChange={(e) => set('audio', e.target.checked)} />
              <span>
                {t('Enregistrer le son', 'Record sound')}
                <span className="block text-xs text-muted">{t('Si la caméra envoie du son : AAC tel quel, G.711 converti en Opus. Modifiable aussi depuis Home Assistant.', 'If the camera sends sound: AAC as is, G.711 converted to Opus. Can also be changed from Home Assistant.')}</span>
              </span>
            </label>
          )}
          {f.record && <DestinationChoice value={f.destinations} onChange={(v) => set('destinations', v)} error={shown('destinations')} />}
          {f.record && (
            <div className="ml-6 grid max-w-md grid-cols-2 gap-3">
              <Field label={t('Conserver (jours)', 'Keep (days)')} htmlFor="cam-days" error={shown('days')} hint={t('Vide : selon le point de stockage.', 'Empty: as set on the storage destination.')}>
                <input id="cam-days" className={inputCls(shown('days'))} inputMode="numeric" placeholder={t('illimité', 'unlimited')} value={f.days} onChange={(e) => set('days', e.target.value)} />
              </Field>
              <Field label={t('Taille max. (Go)', 'Max. size (GB)')} htmlFor="cam-gb" error={shown('gb')} hint={t('Par point de stockage.', 'Per storage destination.')}>
                <input id="cam-gb" className={inputCls(shown('gb'))} inputMode="numeric" placeholder={t('illimitée', 'unlimited')} value={f.gb} onChange={(e) => set('gb', e.target.value)} />
              </Field>
            </div>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-3 sm:col-span-2">
          <Button
            icon={<PlugZap className="size-4" />}
            onClick={() => void runProbe()}
            disabledReason={
              demo
                ? t('Indisponible en mode démo.', 'Not available in demo mode.')
                : probe.kind === 'running'
                  ? t('Test en cours…', 'Testing…')
                  : null
            }
          >
            {f.source === 'usb' ? t('Voir l’image', 'Show the picture') : t('Tester la connexion', 'Test the connection')}
          </Button>
          <span className="text-xs text-muted">
            {f.source === 'usb'
              ? t('Ouvre la webcam avec ces réglages pendant une minute, sans rien enregistrer.', 'Opens the webcam with these settings for a minute, without recording anything.')
              : t("Vérifie l'URL, les identifiants et le codec avant d'enregistrer.", "Checks the URL, the credentials and the codec before saving.")}
          </span>
        </div>
        <div className="sm:col-span-2">
          <ProbeOutcome state={probe} />
        </div>
        <button type="submit" className="hidden" />
      </form>

      {saveError && <p className="mt-4 rounded-md border border-err/40 bg-err/10 px-3 py-2 text-sm text-err">{t("Impossible d'enregistrer : ", "Cannot save: ")}{saveError}</p>}

      {toml && (
        <div className="mt-5 space-y-2">
          <div className="flex items-center justify-between">
            <p className="text-sm font-medium">{t('Bloc pour config.toml', 'Block for config.toml')}</p>
            <Button
              size="sm"
              icon={copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
              onClick={() => void navigator.clipboard?.writeText(toml).then(() => setCopied(true))}
              disabledReason={navigator.clipboard ? null : t('Presse-papiers indisponible (contexte non sécurisé) : sélectionnez le texte.', 'Clipboard unavailable (insecure context): select the text.')}
            >
              {copied ? t('Copié', 'Copied') : t('Copier', 'Copy')}
            </Button>
          </div>
          <pre className="overflow-x-auto rounded-md border border-line bg-bg-2 p-3 text-xs">{toml}</pre>
          {f.password && (
            <p className="text-xs text-muted">
              {t("Définissez le mot de passe dans l'environnement de RustNVR : ", "Set the password in the environment of RustNVR: ")}<code className="text-fg">{envVarName(f.id)}</code>{t(". Il n'apparaît pas ci-dessus.", ". It does not appear above.")}
            </p>
          )}
        </div>
      )}
    </Modal>
    <ConfirmDialog
      open={open && confirmDiscard}
      title={t('Abandonner la saisie ?', 'Discard the changes?')}
      message={t('Les réglages de cette caméra ne sont pas enregistrés : ils seront perdus.', 'The settings of this camera are not saved: they will be lost.')}
      confirmLabel={t('Abandonner', 'Discard')}
      danger
      onConfirm={close}
      onCancel={() => setConfirmDiscard(false)}
    />
    </>
  )
}
