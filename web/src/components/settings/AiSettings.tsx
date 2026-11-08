import { Download, Gpu, PlugZap, ScanSearch, Save } from 'lucide-react'
import { useEffect, useState } from 'react'
import { describeError } from '../../api/client'
import { downloadAiModel, getAi, probeAi, setAi, testAi } from '../../api/endpoints'
import { useReadOnly } from '../../auth/AuthGate'
import { useCapability, useIsDemo, useLive } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { formatBytes, formatPercent } from '../../lib/format'
import { gpuStatusHelp, gpuStatusLabel } from '../../lib/gpu'
import { OBJECT_GROUPS, OTHER_OBJECTS, objectLabel } from '../../lib/objects'
import type { AiModel, AiSettings as Settings, AiTestResult, AiView } from '../../types/api'
import { Button } from '../ui/Button'
import { Badge } from '../ui/Card'
import { ErrorState, LoadingState } from '../ui/States'
import { t } from '../../lib/i18n'
import { AiReplay } from './AiReplay'

const inputCls = 'mt-1 h-9 w-full rounded-md border border-line bg-bg-2 px-3 text-sm text-fg outline-none focus:border-accent disabled:opacity-60'

type Kind = 'none' | 'onnx' | 'http'

interface Form {
  kind: Kind
  model: string
  url: string
  apiKeyEnv: string
  minScore: number
  labels: string[]
  maxPictures: string
  interval: string
  timeout: string
  keepPictures: boolean
  /** A higher confidence at night. */
  night: boolean
  nightScore: number
  nightFrom: string
  nightTo: string
}

function toForm(s: Settings): Form {
  const p = s.provider
  return {
    kind: !p ? 'none' : p.type === 'onnx' ? 'onnx' : 'http',
    model: p?.type === 'onnx' ? p.model : '',
    url: p?.type === 'deepstack_http' ? p.url : '',
    apiKeyEnv: p?.type === 'deepstack_http' ? (p.api_key_env ?? '') : '',
    minScore: s.min_score,
    labels: s.labels,
    maxPictures: String(s.max_pictures),
    interval: String(s.interval_seconds),
    timeout: String(s.timeout_seconds),
    keepPictures: s.keep_pictures ?? false,
    night: Boolean(s.night),
    nightScore: s.night?.min_score ?? Math.min(0.95, s.min_score + 0.2),
    nightFrom: s.night?.from ?? '20:00',
    nightTo: s.night?.to ?? '07:00',
  }
}

function toSettings(f: Form): Settings {
  const base = {
    min_score: f.minScore,
    labels: f.labels,
    max_pictures: Number(f.maxPictures),
    interval_seconds: Number(f.interval),
    timeout_seconds: Number(f.timeout),
    keep_pictures: f.keepPictures,
    night: f.night ? { min_score: f.nightScore, from: f.nightFrom, to: f.nightTo } : null,
  }
  if (f.kind === 'onnx') return { ...base, provider: { type: 'onnx', model: f.model.trim() } }
  if (f.kind === 'http') return { ...base, provider: { type: 'deepstack_http', url: f.url.trim(), ...(f.apiKeyEnv.trim() ? { api_key_env: f.apiKeyEnv.trim() } : {}) } }
  return base
}

/** A model already on the server, the most accurate one (the list goes from
 * the lightest to the most accurate); empty when none is downloaded. */
function downloadedModel(models: AiModel[]): string {
  return [...models].reverse().find((m) => m.downloaded)?.path ?? ''
}

function problems(f: Form): string | null {
  const int = (v: string, min: number, max: number) => Number.isInteger(Number(v)) && Number(v) >= min && Number(v) <= max
  if (f.kind === 'onnx' && !f.model.trim()) return t('Indiquez le fichier du modèle.', 'Enter the model file.')
  if (f.kind === 'http' && !/^https?:\/\/.+/.test(f.url.trim())) return t('Adresse du serveur : http://… ou https://…', 'Server address: http://… or https://…')
  if (!int(f.maxPictures, 1, 20)) return t('Images par événement : entre 1 et 20.', 'Pictures per event: between 1 and 20.')
  if (!int(f.interval, 1, 60)) return t('Intervalle : entre 1 et 60 s.', 'Interval: between 1 and 60 s.')
  if (!int(f.timeout, 1, 120)) return t('Délai maximal : entre 1 et 120 s.', 'Timeout: between 1 and 120 s.')
  return null
}

/** Checks the server address typed in (not stored) with a test pattern. */
function ServerProbe({ url, apiKeyEnv, disabled }: { url: string; apiKeyEnv: string; disabled: boolean }) {
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null)
  // A new address: the previous answer no longer applies.
  useEffect(() => setResult(null), [url, apiKeyEnv])
  const urlOk = /^https?:\/\/.+/.test(url.trim())
  const run = async () => {
    setBusy(true)
    setResult(null)
    try {
      const r = await probeAi(url.trim(), apiKeyEnv.trim() || null)
      setResult(
        r.ok
          ? { ok: true, text: t(`Serveur trouvé, détection d’objets en état de marche (${r.ms} ms${r.device ? `, calcul : ${r.device}` : ''}).`, `Server found, object detection working (${r.ms} ms${r.device ? `, computing: ${r.device}` : ''}).`) }
          : { ok: false, text: r.error ?? t('Échec', 'Failed') },
      )
    } catch (e) {
      setResult({ ok: false, text: describeError(e) })
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="flex flex-wrap items-center gap-2 sm:col-span-2">
      <Button size="sm" icon={<PlugZap className="size-3.5" />} disabledReason={disabled ? t('Lecture seule', 'Read only') : !urlOk ? t('Adresse http://… ou https://… attendue', 'http://… or https://… address expected') : busy ? t('Test en cours… (jusqu’à 30 s au premier appel)', 'Testing… (up to 30 s on the first call)') : null} onClick={() => void run()}>
        {busy ? t('Test…', 'Testing…') : t('Tester l’adresse', 'Test the address')}
      </Button>
      {result ? (
        <span className={`text-xs ${result.ok ? 'text-ok' : 'text-err'}`}>{result.text}</span>
      ) : (
        <span className="text-xs text-muted">{t('Envoie une mire au serveur, sans rien enregistrer.', 'Sends a test pattern to the server, without saving anything.')}</span>
      )}
    </div>
  )
}

/** Graphics cards seen by RustNVR, and how to run the AI on one of them. */
function GpuPanel({ kind, active, device, onUseServer, disabled }: { kind: Kind; active: string | null; device: string | null | undefined; onUseServer: () => void; disabled: boolean }) {
  const all = useLive((s) => s.system?.gpus)
  const gpus = all ?? []
  const usable = gpus.some((g) => g.status === 'usable')
  // `device` is that of the provider running, not of the one chosen in the form.
  const serverRunning = active === 'deepstack_http'
  const onGpu = serverRunning && device != null && /gpu|cuda|directml|openvino|rocm/i.test(device)
  return (
    <div className="max-w-2xl space-y-3 rounded-md border border-line p-4 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <Gpu className="size-4 text-muted" aria-hidden />
        <span className="font-medium">{t('Cartes graphiques', 'Graphics cards')}</span>
        {onGpu && <Badge tone="ok">{t('IA sur GPU', 'AI on GPU')}</Badge>}
      </div>
      {all === undefined ? (
        <p className="text-xs text-muted">{t('Ce serveur ne signale pas ses cartes graphiques (version plus ancienne).', 'This server does not report its graphics cards (older version).')}</p>
      ) : gpus.length === 0 ? (
        <p className="text-xs text-muted">{t('Aucune carte graphique visible : machine virtuelle sans carte transmise (passthrough), ou système autre que Linux.', 'No graphics card visible: virtual machine without a passed-through card, or a system other than Linux.')}</p>
      ) : (
        <ul className="divide-y divide-line">
          {gpus.map((g) => {
            const help = gpuStatusHelp(g)
            return (
              <li key={g.id} className="space-y-0.5 py-2 first:pt-0 last:pb-0">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="font-medium">{g.name}</span>
                  <Badge tone={g.status === 'usable' ? 'ok' : g.status === 'display_only' ? 'neutral' : 'warn'}>{gpuStatusLabel(g)}</Badge>
                  {g.busy_percent !== null && <span className="tabular text-xs text-muted">{t('occupée à ', 'busy ')}{formatPercent(g.busy_percent)}</span>}
                </div>
                <p className="font-mono text-xs text-muted">
                  {[g.device, g.driver && t(`pilote ${g.driver}`, `driver ${g.driver}`)].filter(Boolean).join(' · ')}
                </p>
                {help && <p className="text-xs text-muted">{help}</p>}
              </li>
            )
          })}
        </ul>
      )}
      <p className="text-xs text-muted">
        {t('La reconnaissance intégrée calcule toujours sur le processeur (un cœur par image). Pour l’IA sur GPU, faites tourner CodeProject.AI avec accès à la carte (image « cuda » pour NVIDIA), puis choisissez-le comme fournisseur : RustNVR indique ensuite où chaque analyse a été faite.', 'The built-in recognition always computes on the processor (one core per picture). For AI on a GPU, run CodeProject.AI with access to the card (« cuda » image for NVIDIA), then choose it as provider: RustNVR then shows where each analysis ran.')}
      </p>
      {kind === 'http' && serverRunning && device && (
        <p className="text-xs">
          {t('Dernière analyse de CodeProject.AI : ', 'Last CodeProject.AI analysis: ')}
          <span className={onGpu ? 'font-medium text-ok' : 'font-medium'}>{device}</span>
          {!onGpu && t(' — vérifiez dans CodeProject.AI que le module de détection utilise la carte graphique.', ' — check in CodeProject.AI that the detection module uses the graphics card.')}
        </p>
      )}
      {kind === 'http' && serverRunning && !device && (
        <p className="text-xs text-muted">{t('Où CodeProject.AI calcule (CPU ou GPU) s’affichera après sa première analyse (bouton « Analyser » plus bas).', 'Where CodeProject.AI computes (CPU or GPU) will show after its first analysis (« Analyse » button below).')}</p>
      )}
      {kind === 'http' && !serverRunning && (
        <p className="text-xs text-warn">{t('CodeProject.AI n’est pas encore le fournisseur actif : indiquez l’adresse d’un serveur CodeProject.AI qui fonctionne, puis « Enregistrer et appliquer ».', 'CodeProject.AI is not the running provider yet: enter the address of a working CodeProject.AI server, then « Save and apply ».')}</p>
      )}
      {kind !== 'http' && (
        <Button size="sm" icon={<Gpu className="size-3.5" />} disabledReason={disabled ? t('Lecture seule', 'Read only') : null} onClick={onUseServer}>
          {t('Passer l’IA par le GPU (CodeProject.AI)', 'Run the AI on the GPU (CodeProject.AI)')}
        </Button>
      )}
      {kind !== 'http' && !usable && gpus.length > 0 && (
        <p className="text-xs text-muted">{t('Aucune carte n’est utilisable par RustNVR lui-même, mais CodeProject.AI peut tourner sur une autre machine ou dans un autre conteneur qui y a accès.', 'No card is usable by RustNVR itself, but CodeProject.AI can run on another machine or in another container that has access to one.')}</p>
      )}
    </div>
  )
}

function Status({ v }: { v: AiView }) {
  if (v.active) return <Badge tone="ok">{t('Active (', 'Active (')}{v.active === 'onnx' ? t('intégrée', 'built-in') : 'CodeProject.AI'})</Badge>
  if (v.error) return <Badge tone="err">{t('Erreur', 'Error')}</Badge>
  return <Badge>{t('Désactivée', 'Off')}</Badge>
}

/** Choice of the objects to recognize, by family. */
function ObjectChoice({ value, onChange, disabled }: { value: string[]; onChange: (v: string[]) => void; disabled: boolean }) {
  const [others, setOthers] = useState(value.some((l) => OTHER_OBJECTS.includes(l)))
  const toggle = (label: string) => onChange(value.includes(label) ? value.filter((l) => l !== label) : [...value, label])
  const chip = (label: string) => (
    <label key={label} className={`inline-flex cursor-pointer items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs ${value.includes(label) ? 'border-accent bg-accent/15 text-fg' : 'border-line text-muted'} ${disabled ? 'pointer-events-none opacity-60' : ''}`}>
      <input type="checkbox" className="size-3.5 accent-accent" checked={value.includes(label)} disabled={disabled} onChange={() => toggle(label)} />
      {objectLabel(label)}
    </label>
  )
  return (
    <div className="space-y-2">
      {OBJECT_GROUPS.map((g) => (
        <div key={g.name}>
          <p className="mb-1 text-xs text-muted">{g.name}</p>
          <div className="flex flex-wrap gap-1.5">{g.labels.map(chip)}</div>
        </div>
      ))}
      <button type="button" className="text-xs text-accent hover:underline" onClick={() => setOthers(!others)}>
        {others ? t('Masquer les autres objets', 'Hide the other objects') : t(`Autres objets (${OTHER_OBJECTS.length})`, `Other objects (${OTHER_OBJECTS.length})`)}
      </button>
      {others && <div className="flex flex-wrap gap-1.5">{OTHER_OBJECTS.map(chip)}</div>}
      <p className="text-xs text-muted">
        {value.length === 0 ? t('Aucun objet coché : tous les objets reconnus sont gardés.', 'No object ticked: every recognized object is kept.') : t(`${value.length} objet(s) gardé(s). Chaque caméra peut en garder une partie (onglet Détection).`, `${value.length} object(s) kept. Each camera can keep some of them (Detection tab).`)}
      </p>
    </div>
  )
}

/** Choice of a model the server can download: download it if needed, then use it. */
function ModelPicker({ models, current, disabled, onDownloaded, onUse }: { models: AiModel[]; current: string; disabled: boolean; onDownloaded: (m: AiModel) => void; onUse: (path: string) => void }) {
  // The model in use, if it is one of the list; the lightest otherwise.
  const [id, setId] = useState(() => models.find((m) => m.path === current)?.id ?? models[0]?.id ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [done, setDone] = useState<string | null>(null)
  const m = models.find((x) => x.id === id)
  if (!m) return null
  const download = async () => {
    setBusy(true)
    setError(null)
    setDone(null)
    try {
      const r = await downloadAiModel(m.id)
      onDownloaded(r)
      onUse(r.path)
      setDone(t(`${r.name} téléchargé et vérifié. Cliquez sur « Enregistrer » pour l’utiliser.`, `${r.name} downloaded and checked. Click « Save » to use it.`))
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="max-w-xl space-y-2 rounded-md border border-line p-3 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <select
          className="h-9 rounded-md border border-line bg-bg-2 px-2 text-sm"
          aria-label={t('Modèle', 'Model')}
          value={id}
          disabled={busy}
          onChange={(e) => {
            setId(e.target.value)
            setError(null)
            setDone(null)
          }}
        >
          {models.map((x) => (
            <option key={x.id} value={x.id}>
              {x.name} — {formatBytes(x.bytes)}
              {x.downloaded ? t(' (téléchargé)', ' (downloaded)') : ''}
            </option>
          ))}
        </select>
        {!m.downloaded ? (
          <Button icon={<Download className="size-4" />} disabledReason={disabled ? t('Lecture seule', 'Read only') : busy ? t('Téléchargement en cours…', 'Downloading…') : null} onClick={() => void download()}>
            {busy ? t('Téléchargement…', 'Downloading…') : t('Télécharger', 'Download')}
          </Button>
        ) : current === m.path ? (
          <Badge tone="ok">{t('Choisi', 'Chosen')}</Badge>
        ) : (
          <Button disabledReason={disabled ? t('Lecture seule', 'Read only') : null} onClick={() => onUse(m.path)}>
            {t('Utiliser ce modèle', 'Use this model')}
          </Button>
        )}
      </div>
      <p className="text-xs text-muted">{m.hint}</p>
      <p className="text-xs text-muted">
        {t('Licence ', 'Licence ')}{m.license}
        {m.downloaded ? t(` · sur le serveur : ${m.path}`, ` · on the server: ${m.path}`) : ''}
      </p>
      {done && <p className="text-xs text-ok">{done}</p>}
      {error && <p className="text-xs text-err">{error}</p>}
      <p className="text-xs text-muted">{t('Téléchargé par le serveur depuis la publication officielle du modèle (Internet nécessaire), puis vérifié par son empreinte SHA-256 avant d’être utilisé. Temps mesurés sur un cœur de processeur de bureau ; l’analyse n’a lieu que sur les images des mouvements détectés.', 'Downloaded by the server from the official release of the model (Internet needed), then checked by its SHA-256 fingerprint before use. Times measured on one core of a desktop processor; analysis only runs on the pictures of detected motion.')}</p>
    </div>
  )
}

/** Analyses a current picture of a camera and shows what the AI sees. */
function AiTest({ active }: { active: boolean }) {
  const cameras = useLive((s) => s.cameras).filter((c) => c.status === 'online')
  const [camera, setCamera] = useState('')
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<AiTestResult | null>(null)
  const [error, setError] = useState<string | null>(null)
  const id = camera || cameras[0]?.id || ''
  const run = async () => {
    setBusy(true)
    setError(null)
    try {
      setResult(await testAi(id))
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="space-y-3 rounded-md border border-line p-4">
      <div className="flex flex-wrap items-end gap-2">
        <label className="text-sm">
          <span className="text-xs text-muted">{t('Essayer sur une capture de', 'Try on a snapshot of')}</span>
          <select className={`${inputCls} w-48`} value={id} onChange={(e) => setCamera(e.target.value)}>
            {cameras.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
        </label>
        <Button icon={<ScanSearch className="size-4" />} disabledReason={!active ? t('Enregistrez d’abord un fournisseur qui fonctionne.', 'First save a provider that works.') : !id ? t('Aucune caméra en ligne.', 'No camera online.') : busy ? t('Analyse en cours…', 'Analysing…') : null} onClick={() => void run()}>
          {t('Analyser', 'Analyse')}
        </Button>
      </div>
      {error && <p className="text-sm text-err">{error}</p>}
      {result && (
        <div className="grid gap-3 lg:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
          <div className="relative overflow-hidden rounded bg-black">
            <img src={result.image} alt={t('Image analysée', 'Analysed picture')} className="w-full" />
            {result.objects.map((o, i) =>
              o.bbox ? (
                <div
                  key={i}
                  className={`pointer-events-none absolute rounded-sm border-2 ${o.kept ? 'border-ok' : 'border-muted/60 border-dashed'}`}
                  style={{ left: `${o.bbox[0] * 100}%`, top: `${o.bbox[1] * 100}%`, width: `${(o.bbox[2] - o.bbox[0]) * 100}%`, height: `${(o.bbox[3] - o.bbox[1]) * 100}%` }}
                >
                  <span className={`absolute -top-5 left-0 rounded px-1 text-[10px] font-semibold whitespace-nowrap ${o.kept ? 'bg-ok text-bg-2' : 'bg-black/70 text-muted'}`}>
                    {objectLabel(o.label)} {Math.round(o.score * 100)} %
                  </span>
                </div>
              ) : null,
            )}
          </div>
          <div className="text-sm">
            <p className="mb-2 text-xs text-muted">
              {t('Analyse en ', 'Analysed in ')}
              {result.ms}
              {t(' ms', ' ms')}
              {result.device && t(` · calcul : ${result.device}`, ` · computing: ${result.device}`)}
            </p>
            {result.objects.length === 0 ? (
              <p className="text-muted">{t('Rien de reconnu sur cette image.', 'Nothing recognized in this picture.')}</p>
            ) : (
              <ul className="space-y-1">
                {result.objects.map((o, i) => (
                  <li key={i} className={o.kept ? 'text-ok' : 'text-muted'}>
                    {objectLabel(o.label)} — {Math.round(o.score * 100)} %{o.zone ? ` (${o.zone})` : ''}{o.kept ? '' : t(' (écarté par les réglages)', ' (left out by the settings)')}
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      )}
    </div>
  )
}

/** Paramètres → Intelligence artificielle. */
export function AiSettings() {
  const demo = useIsDemo()
  const writable = useCapability('settings_write') === true
  const viewerReason = useReadOnly()
  const view = useAsync(() => (demo || !writable ? Promise.resolve(null) : getAi()), [demo, writable])
  const [saved, setSaved] = useState<AiView | null>(null)
  const [form, setForm] = useState<Form | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const v = saved ?? view.data
  useEffect(() => {
    if (v && form === null) setForm(toForm(v.settings))
  }, [v, form])

  if (demo) return <p className="px-5 py-4 text-sm text-muted">{t('Indisponible en mode démo.', 'Not available in demo mode.')}</p>
  if (!writable) return <p className="px-5 py-4 text-sm text-muted">{t("Ce serveur ne permet pas de modifier ces réglages depuis l'interface.", "This server does not allow changing these settings from the interface.")}</p>
  if (view.error && !v) return <ErrorState error={view.error} onRetry={view.reload} compact />
  if (!v || !form) return <LoadingState compact />

  const set = <K extends keyof Form>(k: K, value: Form[K]) => setForm({ ...form, [k]: value })
  const problem = problems(form)
  const changed = JSON.stringify(toSettings(form)) !== JSON.stringify(toSettings(toForm(v.settings)))
  const disabled = viewerReason !== null
  const save = async () => {
    setBusy(true)
    setError(null)
    try {
      const next = await setAi(toSettings(form))
      setSaved(next)
      setForm(toForm(next.settings))
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-5 px-5 py-4">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        {t('État : ', 'State: ')}<Status v={v} />
        {v.active && v.device && <span className="text-xs text-muted">{t('Calcul : ', 'Computing: ')}<span className="font-medium text-fg">{v.device}</span></span>}
        {v.error && <span className="text-xs text-err">{v.error}</span>}
        {!v.detection_ready && <span className="text-xs text-warn">{t('La reconnaissance nécessite la détection de mouvement (FFmpeg et base de données).', 'Recognition needs motion detection (FFmpeg and database).')}</span>}
      </div>

      <fieldset className="space-y-2 text-sm" disabled={disabled}>
        <legend className="mb-1 font-medium">{t('Fournisseur', 'Provider')}</legend>
        <label className="flex items-start gap-2">
          <input type="radio" name="ai-kind" className="mt-1 accent-accent" checked={form.kind === 'none'} onChange={() => set('kind', 'none')} />
          <span>{t('Désactivée', 'Off')}<span className="block text-xs text-muted">{t('Aucune reconnaissance : seule la détection de mouvement est utilisée.', 'No recognition: only motion detection is used.')}</span></span>
        </label>
        <label className={`flex items-start gap-2 ${v.onnx_available ? '' : 'opacity-50'}`}>
          <input type="radio" name="ai-kind" className="mt-1 accent-accent" disabled={!v.onnx_available} checked={form.kind === 'onnx'} onChange={() => setForm({ ...form, kind: 'onnx', model: form.model.trim() || downloadedModel(v.models) })} />
          <span>
            {t('Intégrée (recommandée)', 'Built-in (recommended)')}
            <span className="block text-xs text-muted">{v.onnx_available ? t('Modèle YOLOX exécuté par RustNVR, sans autre programme (≈ 40 à 400 ms par image selon le modèle).', 'YOLOX model run by RustNVR, no other program (≈ 40 to 400 ms per picture depending on the model).') : t('Absente de cette version de RustNVR (compilée sans la fonction « onnx »).', 'Missing from this build of RustNVR (compiled without the « onnx » feature).')}</span>
          </span>
        </label>
        <label className="flex items-start gap-2">
          <input type="radio" name="ai-kind" className="mt-1 accent-accent" checked={form.kind === 'http'} onChange={() => set('kind', 'http')} />
          <span>CodeProject.AI / DeepStack<span className="block text-xs text-muted">{t('Serveur de reconnaissance installé à part, interrogé par HTTP.', 'Recognition server installed separately, queried over HTTP.')}</span></span>
        </label>
      </fieldset>

      <GpuPanel kind={form.kind} active={v.active} device={v.device} disabled={disabled} onUseServer={() => setForm({ ...form, kind: 'http', url: form.url || 'http://127.0.0.1:32168' })} />

      {form.kind === 'onnx' && (
        <div className="space-y-3">
          <ModelPicker
            models={v.models}
            current={form.model.trim()}
            disabled={disabled}
            onDownloaded={(m) => setSaved({ ...v, models: v.models.map((x) => (x.id === m.id ? m : x)) })}
            onUse={(path) => set('model', path)}
          />
          <label className="block max-w-xl text-sm">
            {t('Fichier du modèle (sur le serveur)', 'Model file (on the server)')}
            <input className={inputCls} value={form.model} disabled={disabled} placeholder={v.models[0]?.path ?? '/var/lib/rustnvr/models/yolox_nano.onnx'} onChange={(e) => set('model', e.target.value)} />
            <span className="mt-1 block text-xs text-muted">
              {t('Rempli par « Télécharger » ci-dessus, ou le chemin d’un autre modèle YOLOX au format ONNX (yolox_tiny…, licence Apache-2.0) : voir docs/AI.md.', 'Filled in by « Download » above, or the path of another YOLOX model in ONNX format (yolox_tiny…, Apache-2.0 licence): see docs/AI.md.')}
            </span>
          </label>
        </div>
      )}
      {form.kind === 'http' && (
        <div className="grid max-w-xl gap-3 sm:grid-cols-2">
          <label className="text-sm">
            {t('Adresse du serveur', 'Server address')}
            <input className={inputCls} value={form.url} disabled={disabled} placeholder="http://127.0.0.1:32168" onChange={(e) => set('url', e.target.value)} />
          </label>
          <label className="text-sm">
            {t("Clé d'API (variable d'environnement)", "API key (environment variable)")}
            <input className={inputCls} value={form.apiKeyEnv} disabled={disabled} placeholder="facultatif" onChange={(e) => set('apiKeyEnv', e.target.value)} />
          </label>
          <ServerProbe url={form.url} apiKeyEnv={form.apiKeyEnv} disabled={disabled} />
        </div>
      )}

      {form.kind !== 'none' && (
        <>
          <label className="block max-w-md text-sm">
            {t('Confiance minimale : ', 'Minimum confidence: ')}<span className="tabular text-fg">{Math.round(form.minScore * 100)} %</span>
            <input type="range" min={0.2} max={0.95} step={0.05} className="mt-1 w-full accent-accent" value={form.minScore} disabled={disabled} onChange={(e) => set('minScore', Number(e.target.value))} />
            <span className="block text-xs text-muted">{t('Plus haut : moins de fausses détections, mais des objets ratés. 50 % convient au modèle intégré.', 'Higher: fewer false detections, but missed objects. 50 % suits the built-in model.')}</span>
          </label>

          <div className="max-w-md space-y-2 text-sm">
            <label className="flex items-start gap-2">
              <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={form.night} disabled={disabled} onChange={(e) => set('night', e.target.checked)} />
              <span>
                {t('Plus exigeante la nuit', 'Stricter at night')}
                <span className="block text-xs text-muted">{t('En infrarouge, insectes, pluie et phares trompent plus souvent l’IA : une confiance plus haute pendant ces heures (heure du serveur).', 'In infrared light, insects, rain and headlights fool the AI more often: a higher confidence during these hours (server time).')}</span>
              </span>
            </label>
            {form.night && (
              <div className="ml-6 space-y-2">
                <label className="block">
                  {t('Confiance minimale la nuit : ', 'Minimum confidence at night: ')}<span className="tabular text-fg">{Math.round(form.nightScore * 100)} %</span>
                  <input type="range" min={0.2} max={0.95} step={0.05} className="mt-1 w-full accent-accent" value={form.nightScore} disabled={disabled} onChange={(e) => set('nightScore', Number(e.target.value))} />
                </label>
                <div className="flex flex-wrap items-center gap-2">
                  {t('De', 'From')}
                  <input type="time" aria-label={t('Début de la nuit', 'Start of the night')} className="h-8 rounded-md border border-line bg-bg-2 px-2" value={form.nightFrom} disabled={disabled} onChange={(e) => set('nightFrom', e.target.value)} />
                  {t('à', 'to')}
                  <input type="time" aria-label={t('Fin de la nuit', 'End of the night')} className="h-8 rounded-md border border-line bg-bg-2 px-2" value={form.nightTo} disabled={disabled} onChange={(e) => set('nightTo', e.target.value)} />
                </div>
              </div>
            )}
          </div>

          <div className="text-sm">
            <p className="mb-2 font-medium">{t('Objets à reconnaître', 'Objects to recognize')}</p>
            <ObjectChoice value={form.labels} onChange={(l) => set('labels', l)} disabled={disabled} />
          </div>

          <label className="flex max-w-xl items-start gap-2 text-sm">
            <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={form.keepPictures} disabled={disabled} onChange={(e) => set('keepPictures', e.target.checked)} />
            <span>
              {t('Garder les images analysées', 'Keep the analysed pictures')}
              <span className="block text-xs text-muted">{t('Chaque image envoyée à l’IA, avec ce qu’elle y a trouvé, dans la destination par défaut (dossier .captures) : pour rejouer l’IA dessus ci-dessous et voir si elle détecte bien. Supprimées avec les vidéos (même nombre de jours). Quelques Mo par jour et par caméra.', 'Every picture sent to the AI, with what it found, in the default destination (.captures folder): to replay the AI on them below and see whether it detects well. Deleted with the videos (same number of days). A few MB per day and camera.')}</span>
            </span>
          </label>

          <details className="text-sm">
            <summary className="cursor-pointer text-muted">{t('Réglages avancés', 'Advanced settings')}</summary>
            <div className="mt-2 grid max-w-xl gap-3 sm:grid-cols-3">
              <label>
                {t('Images par événement', 'Pictures per event')}
                <input className={inputCls} inputMode="numeric" value={form.maxPictures} disabled={disabled} onChange={(e) => set('maxPictures', e.target.value)} />
              </label>
              <label>
                {t('Intervalle (s)', 'Interval (s)')}
                <input className={inputCls} inputMode="numeric" value={form.interval} disabled={disabled} onChange={(e) => set('interval', e.target.value)} />
              </label>
              <label>
                {t('Délai maximal (s)', 'Timeout (s)')}
                <input className={inputCls} inputMode="numeric" value={form.timeout} disabled={disabled} onChange={(e) => set('timeout', e.target.value)} />
              </label>
            </div>
            <p className="mt-1 max-w-xl text-xs text-muted">{t("La vignette est analysée 1 s après le début du mouvement, puis une nouvelle image à chaque intervalle tant que rien n'est reconnu.", "The thumbnail is analysed 1 s after motion starts, then a new picture at each interval as long as nothing is recognized.")}</p>
          </details>
        </>
      )}

      {error && <p className="text-sm text-err">{error}</p>}
      <div className="flex flex-wrap items-center gap-3">
        <Button variant="primary" icon={<Save className="size-4" />} disabledReason={viewerReason ?? problem ?? (!changed ? t('Aucun changement.', 'No change.') : busy ? t('Application…', 'Applying…') : null)} onClick={() => void save()}>
          {t('Enregistrer et appliquer', 'Save and apply')}
        </Button>
        <span className="text-xs text-muted">{t('Appliqué sans redémarrage, dans ai.ui.toml (remplace la section [ai] de config.toml).', 'Applied without restart, in ai.ui.toml (replaces the [ai] section of config.toml).')}</span>
      </div>

      {form.kind !== 'none' && <AiTest active={v.active !== null} />}
      {form.kind !== 'none' && <AiReplay active={v.active !== null} />}
    </div>
  )
}
