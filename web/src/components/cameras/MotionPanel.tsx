import { RefreshCw, Save } from 'lucide-react'
import { useState } from 'react'
import { describeError } from '../../api/client'
import { useReadOnly } from '../../auth/AuthGate'
import { getAi, getMotion, setMotion, snapshotUrl } from '../../api/endpoints'
import { useCapability, useData, useIsDemo } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { OBJECT_GROUPS, objectLabel } from '../../lib/objects'
import type { AiView, Camera, MotionSettings } from '../../types/api'
import { Button } from '../ui/Button'
import { Badge } from '../ui/Card'
import { ErrorState, LoadingState } from '../ui/States'
import { CameraPlayer } from './CameraPlayer'
import { LineEditor } from './LineEditor'
import { MaskEditor } from './MaskEditor'
import { ZoneEditor } from './ZoneEditor'
import { t } from '../../lib/i18n'

const DEFAULTS: MotionSettings = { enabled: false, sensitivity: 50, zones: [] }

function sensitivityHelp(s: number): string {
  if (s >= 80) return t('Très sensible : le moindre changement compte (feuillage, pluie, insectes la nuit).', 'Very sensitive: the slightest change counts (leaves, rain, insects at night).')
  if (s >= 40) return t('Équilibrée : une personne ou un véhicule à quelques mètres.', 'Balanced: a person or a vehicle a few metres away.')
  return t('Peu sensible : seuls les grands changements contrastés comptent.', 'Not very sensitive: only large contrasted changes count.')
}

/** Live state of the detection of a camera. */
export function MotionStatus({ camera }: { camera: Camera }) {
  const m = camera.motion
  if (!m.enabled) return <Badge>{t('Détection désactivée', 'Detection off')}</Badge>
  if (m.error) return <Badge tone="err">{t('Détection en erreur', 'Detection error')}</Badge>
  if (!m.running) return <Badge tone="warn">{t('Détection en attente du flux', 'Detection waiting for the stream')}</Badge>
  return m.active ? <Badge tone="warn">{t('Mouvement en cours', 'Motion now')}</Badge> : <Badge tone="ok">{t('Détection active', 'Detection on')}</Badge>
}

/** Snapshot of the camera, refreshed on demand (the zone editor background). */
function Snapshot({ camera, stamp }: { camera: Camera; stamp: number }) {
  const [failed, setFailed] = useState<number | null>(null)
  if (failed === stamp || camera.status !== 'online') {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[linear-gradient(var(--color-line)_1px,transparent_1px),linear-gradient(90deg,var(--color-line)_1px,transparent_1px)] bg-[size:10%_10%] text-xs text-muted">
        {camera.status === 'online' ? t('Capture indisponible', 'Snapshot unavailable') : t('Caméra hors ligne', 'Camera offline')}
      </div>
    )
  }
  return <img src={snapshotUrl(camera.id, stamp)} alt={t(`Image actuelle de ${camera.name}`, `Current picture of ${camera.name}`)} className="h-full w-full object-cover" onError={() => setFailed(stamp)} />
}


/** Objects recognized for this camera, among those of the AI settings. */
/** Objects the AI settings let cameras choose from (empty when the AI is off). */
function offeredObjects(ai: AiView | null): string[] {
  if (!ai?.active) return []
  return ai.settings.labels.length > 0 ? ai.settings.labels : OBJECT_GROUPS.flatMap((g) => g.labels)
}

function CameraObjects({ offered, value, onChange, disabled }: { offered: string[]; value: string[]; onChange: (v: string[]) => void; disabled: boolean }) {
  if (offered.length === 0) {
    return <p className="text-xs text-muted">{t("Reconnaissance d'objets désactivée (Paramètres → Intelligence artificielle).", "Object recognition off (Settings → Artificial intelligence).")}</p>
  }
  const toggle = (label: string) => onChange(value.includes(label) ? value.filter((l) => l !== label) : [...value, label])
  return (
    <div>
      <div className="flex flex-wrap gap-1.5">
        {offered.map((label) => (
          <label key={label} className={`inline-flex cursor-pointer items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs ${value.includes(label) ? 'border-accent bg-accent/15 text-fg' : 'border-line text-muted'} ${disabled ? 'pointer-events-none opacity-60' : ''}`}>
            <input type="checkbox" className="size-3.5 accent-accent" checked={value.includes(label)} disabled={disabled} onChange={() => toggle(label)} />
            {objectLabel(label)}
          </label>
        ))}
      </div>
      <p className="mt-1 text-xs text-muted">{value.length === 0 ? t('Aucun coché : tous les objets des réglages de l’IA.', 'None ticked: all the objects of the AI settings.') : t('Seuls ces objets comptent pour cette caméra (événements, capteur « Personne »).', 'Only these objects count for this camera (events, « Person » sensor).')}</p>
    </div>
  )
}

/** Camera page, Détection tab: enable detection, sensitivity, zones. */
export function MotionPanel({ camera }: { camera: Camera }) {
  const demo = useIsDemo()
  const { source } = useData()
  const available = useCapability('motion_detection') === true
  const viewerReason = useReadOnly()
  const snapshots = useCapability('snapshots') === true
  const ai = useCapability('ai') === true
  const aiView = useAsync(() => (ai && !demo ? getAi() : Promise.resolve(null)), [ai, demo])
  const offered = offeredObjects(aiView.data ?? null)
  const loaded = useAsync(() => (demo || !available ? Promise.resolve(DEFAULTS) : getMotion(camera.id)), [camera.id, demo, available])
  const [draft, setDraft] = useState<MotionSettings | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [shot, setShot] = useState(() => Date.now())

  if (loaded.error) return <ErrorState error={loaded.error} onRetry={loaded.reload} compact />
  if (!loaded.data) return <LoadingState compact />
  const saved = loaded.data
  const s = draft ?? saved
  const dirty = draft !== null && JSON.stringify(draft) !== JSON.stringify(saved)
  const edit = (patch: Partial<MotionSettings>) => {
    setDraft({ ...s, ...patch })
    setError(null)
  }
  const incomplete = s.zones.find((z) => z.points.length < 3)
  const reason = demo
    ? t('Indisponible en mode démo.', 'Not available in demo mode.')
    : viewerReason
      ? viewerReason
    : !available
      ? t("La détection n'est pas disponible sur ce serveur (FFmpeg ou base de données manquant).", "Detection is not available on this server (FFmpeg or database missing).")
      : null

  const save = async () => {
    setBusy(true)
    setError(null)
    try {
      const result = await setMotion(camera.id, s)
      loaded.reload()
      setDraft(null)
      source.store.notify('info', t(`${camera.name} : réglages de détection enregistrés`, `${camera.name}: detection settings saved`), camera.id)
      return result
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="space-y-2">
          <label className="flex items-start gap-2 text-sm">
            <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={s.enabled} disabled={reason !== null} onChange={(e) => edit({ enabled: e.target.checked })} />
            <span>
              {t('Détecter les mouvements sur cette caméra', 'Detect motion on this camera')}
              <span className="block text-xs text-muted">
                {t('Analyse ', 'Analyses ')}{camera.has_sub_stream ? t('du sous-flux', 'the sub stream') : t('du flux principal', 'the main stream')}{t(' quelques fois par seconde, sans IA. Coût : un petit décodage FFmpeg par caméra.', ' a few times per second, without AI. Cost: a small FFmpeg decoding per camera.')}
              </span>
            </span>
          </label>
          <div className="flex flex-wrap items-center gap-2">
            <MotionStatus camera={camera} />
            {camera.motion.error && <span className="text-xs text-err">{camera.motion.error}</span>}
          </div>
        </div>
        <Button
          variant="primary"
          icon={<Save className="size-4" />}
          disabledReason={reason ?? (incomplete ? t(`La zone « ${incomplete.name} » a moins de 3 points.`, `Zone « ${incomplete.name} » has fewer than 3 points.`) : !dirty ? t('Aucun changement.', 'No change.') : busy ? t('Enregistrement…', 'Saving…') : null)}
          onClick={() => void save()}
        >
          {t('Enregistrer', 'Save')}
        </Button>
      </div>
      {camera.motion.paused && (
        <p className="rounded-md border border-warn/40 bg-warn/10 px-3 py-2 text-sm text-warn">
          {t('La détection est en pause sur toutes les caméras (Home Assistant ou le bandeau en haut de la page) : ces réglages s’appliqueront à la reprise.', 'Detection is paused on every camera (Home Assistant or the banner at the top of the page): these settings apply once it resumes.')}
        </p>
      )}
      {camera.recording_enabled && camera.record_mode === 'motion' && !s.enabled && (
        <p className="rounded-md border border-warn/40 bg-warn/10 px-3 py-2 text-sm text-warn">
          {t("Cette caméra enregistre « seulement sur mouvement » : sans détection, rien n'est enregistré.", "This camera records « only on motion »: without detection, nothing is recorded.")}
        </p>
      )}
      {error && <p className="rounded-md border border-err/40 bg-err/10 px-3 py-2 text-sm text-err">{error}</p>}

      <label className="block max-w-md text-xs text-muted">
        {t('Sensibilité ', 'Sensitivity ')}{s.zones.length > 0 ? t('(sans zone)', '(no zone)') : ''} : <span className="tabular text-fg">{s.sensitivity}</span>
        <input type="range" min={1} max={100} className="mt-1 w-full accent-accent" value={s.sensitivity} disabled={reason !== null || s.zones.length > 0} onChange={(e) => edit({ sensitivity: Number(e.target.value) })} />
        <span className="mt-1 block">{s.zones.length > 0 ? t('Avec des zones, chaque zone a sa propre sensibilité.', 'With zones, each zone has its own sensitivity.') : sensitivityHelp(s.sensitivity)}</span>
      </label>

      {ai && !demo && (
        <div>
          <h3 className="mb-2 text-sm font-medium">{t('Objets à reconnaître (IA)', 'Objects to recognize (AI)')}</h3>
          <CameraObjects offered={offered} value={s.objects ?? []} onChange={(objects) => edit({ objects })} disabled={reason !== null} />
        </div>
      )}

      <div>
        <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
          <h3 className="text-sm font-medium">{t('Zones de détection', 'Detection zones')}</h3>
          {!demo && (
            <Button size="sm" icon={<RefreshCw className="size-3.5" />} disabledReason={snapshots ? null : t('Captures indisponibles (FFmpeg manquant).', 'Snapshots unavailable (FFmpeg missing).')} onClick={() => setShot(Date.now())}>
              {t("Rafraîchir l'image", "Refresh the picture")}
            </Button>
          )}
        </div>
        <ZoneEditor
          zones={s.zones}
          onChange={(zones) => edit({ zones })}
          objects={s.objects && s.objects.length > 0 ? s.objects : offered}
          disabled={reason !== null}
          background={demo ? <CameraPlayer camera={camera} active={false} /> : snapshots ? <Snapshot camera={camera} stamp={shot} /> : null}
        />
        {s.zones.length > 0 && (
          <p className="mt-2 text-xs text-muted">
            {t("Avec des zones, rien n'est détecté en dehors : ni mouvement, ni objet reconnu (pieds hors des zones). Les lignes de passage restent actives partout.", "With zones, nothing is detected outside them: neither motion nor recognized objects (feet outside the zones). Crossing lines stay active everywhere.")}
          </p>
        )}
      </div>

      <div>
        <h3 className="mb-2 text-sm font-medium">{t('Lignes de passage', 'Crossing lines')}</h3>
        <LineEditor
          lines={s.lines ?? []}
          onChange={(lines) => edit({ lines })}
          objects={offered}
          disabled={reason !== null}
          background={demo ? <CameraPlayer camera={camera} active={false} /> : snapshots ? <Snapshot camera={camera} stamp={shot} /> : null}
        />
      </div>

      <div>
        <h3 className="mb-2 text-sm font-medium">{t('Masques de confidentialité', 'Privacy masks')}</h3>
        <MaskEditor
          masks={s.masks ?? []}
          onChange={(masks) => edit({ masks })}
          disabled={reason !== null}
          background={demo ? <CameraPlayer camera={camera} active={false} /> : snapshots ? <Snapshot camera={camera} stamp={shot} /> : null}
        />
      </div>
    </div>
  )
}
