import { Save } from 'lucide-react'
import { useState } from 'react'
import { describeError } from '../../api/client'
import { useReadOnly } from '../../auth/AuthGate'
import { getSettings, setSettings } from '../../api/endpoints'
import { useCapability, useIsDemo, useLive } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import type { GeneralSettings, HwAccel } from '../../types/api'
import { Button } from '../ui/Button'
import { ErrorState, LoadingState } from '../ui/States'
import { ConfigKey } from './SettingsSection'
import { t } from '../../lib/i18n'

export type GeneralSection = 'recording' | 'motion' | 'live' | 'storage'

interface FieldDef {
  /** Path in GeneralSettings: `recording.segment_seconds`, `live_max_bitrate_kbps`… */
  key: string
  label: string
  help: string
  min: number
  max: number
  unit: string
}

const FIELDS: Record<GeneralSection, FieldDef[]> = {
  recording: [
    { key: 'recording.segment_seconds', label: t('Durée des segments', 'Segment length'), help: t('Taille des fichiers ; coupés sur une image clé.', 'File size; cut on a key frame.'), min: 5, max: 3600, unit: 's' },
    { key: 'recording.fragment_millis', label: t('Durée des fragments', 'Fragment length'), help: t('Perte maximale ≈ 2 × cette durée en cas de crash.', 'At most ≈ 2 × this length lost on a crash.'), min: 100, max: 10000, unit: 'ms' },
    { key: 'recording.pre_roll_seconds', label: t('Avant un mouvement', 'Before motion'), help: t('Enregistrement « sur mouvement » : vidéo gardée avant chaque événement.', '« On motion » recording: video kept before each event.'), min: 0, max: 60, unit: 's' },
    { key: 'recording.post_roll_seconds', label: t('Après un mouvement', 'After motion'), help: t('Enregistrement « sur mouvement » : vidéo gardée après chaque événement.', '« On motion » recording: video kept after each event.'), min: 0, max: 600, unit: 's' },
    { key: 'recording.queue_frames', label: t("File d'images", "Frame queue"), help: t("Images en attente avant l'enregistreur (avancé).", "Frames waiting before the recorder (advanced)."), min: 8, max: 8192, unit: 'images' },
    { key: 'recording.max_timestamp_jump_seconds', label: t("Saut d'horodatage max.", "Max. timestamp jump"), help: t('Au-delà, un nouveau segment commence (avancé).', 'Beyond it, a new segment starts (advanced).'), min: 1, max: 300, unit: 's' },
    { key: 'recording.retry_after_error_seconds', label: t('Nouvel essai après erreur', 'Retry after error'), help: t("Délai après une erreur d'écriture, doublé à chaque échec (avancé).", "Delay after a write error, doubled on each failure (advanced)."), min: 1, max: 600, unit: 's' },
  ],
  motion: [
    { key: 'motion.fps', label: t('Images analysées', 'Analysed pictures'), help: t('Par seconde ; plus = plus réactif, plus de CPU.', 'Per second; more = more responsive, more CPU.'), min: 1, max: 15, unit: 'i/s' },
    { key: 'motion.width', label: t("Largeur d'analyse", "Analysis width"), help: t('Plus = petits objets mieux vus, plus de CPU.', 'More = small objects better seen, more CPU.'), min: 160, max: 640, unit: 'px' },
    { key: 'motion.post_seconds', label: t("Fin d'un événement", "End of an event"), help: t('Après ce délai sans mouvement.', 'After this long without motion.'), min: 1, max: 300, unit: 's' },
    { key: 'motion.max_event_seconds', label: t('Durée max. d’un événement', 'Max. event length'), help: t('Les événements plus longs sont découpés.', 'Longer events are split.'), min: 30, max: 3600, unit: 's' },
    { key: 'motion.events_retention_days', label: t('Conservation des événements', 'Event retention'), help: t('Vignettes comprises.', 'Thumbnails included.'), min: 1, max: 3650, unit: 'jours' },
  ],
  live: [{ key: 'live_max_bitrate_kbps', label: t('Débit de la conversion H.264', 'H.264 conversion bitrate'), help: t('Pour les navigateurs sans H.265 (Firefox).', 'For browsers without H.265 (Firefox).'), min: 100, max: 20000, unit: 'kbit/s' }],
  storage: [{ key: 'low_space_percent', label: t("Seuil d'alerte d'espace disque", "Disk space warning threshold"), help: t("Au-delà, l'interface et Home Assistant signalent un manque d'espace.", "Beyond it, the interface and Home Assistant report a lack of space."), min: 50, max: 99, unit: '%' }],
}

const NOTE: Record<GeneralSection, string> = {
  recording: t('Enregistrer redémarre les caméras (quelques secondes sans enregistrement).', 'Saving restarts the cameras (a few seconds without recording).'),
  motion: t('Enregistrer redémarre la détection des caméras concernées.', 'Saving restarts detection on the cameras concerned.'),
  live: t("S'applique aux prochaines conversions ; changer l'accélération matérielle relance l'essai sur la carte.", "Applies to the next conversions; changing the hardware acceleration runs the test on the card again."),
  storage: t('Contrôlé chaque minute.', 'Checked every minute.'),
}

function read(s: GeneralSettings, key: string): number {
  const [a = '', b] = key.split('.')
  const v = b ? (s as unknown as Record<string, Record<string, number>>)[a]?.[b] : (s as unknown as Record<string, number>)[a]
  return v ?? 0
}

function write(s: GeneralSettings, key: string, v: number): GeneralSettings {
  const [a = '', b] = key.split('.')
  if (!b) return { ...s, [a]: v }
  return { ...s, [a]: { ...(s as unknown as Record<string, object>)[a], [b]: v } }
}

/** Where the H.264 conversions run now. */
function HwStatus({ hw }: { hw: HwAccel }) {
  const how = hw.api === 'nvenc' ? 'NVENC' : ['VAAPI', hw.device, hw.driver].filter(Boolean).join(', ')
  const [tone, text] = ((): [string, string] => {
    switch (hw.state) {
      case 'active':
        return ['text-ok', t(`Conversions sur la carte graphique : ${hw.card ?? ''} (${how}).`, `Conversions on the graphics card: ${hw.card ?? ''} (${how}).`)]
      case 'testing':
        return ['text-muted', t('Essai de conversion sur la carte graphique en cours…', 'Test conversion on the graphics card running…')]
      case 'no_card':
        return ['text-muted', t('Aucune carte graphique utilisable par RustNVR (NVIDIA, Intel ou AMD) : conversions sur le processeur (voir Paramètres → Intelligence artificielle → Cartes graphiques).', 'No graphics card usable by RustNVR (NVIDIA, Intel or AMD): conversions on the processor (see Settings → Artificial intelligence → Graphics cards).')]
      case 'failed':
        return ['text-warn', t('L’essai sur la carte graphique a échoué : conversions sur le processeur.', 'The test on the graphics card failed: conversions on the processor.')]
      default:
        return ['text-muted', t('Conversions sur le processeur.', 'Conversions on the processor.')]
    }
  })()
  return (
    <p className={`pl-6 text-xs ${tone}`}>
      {text}
      {hw.detail && <span className="block text-muted">{hw.detail}</span>}
    </p>
  )
}

/** Editable general settings of one section (saved in settings.ui.toml). */
export function GeneralSettingsForm({ section }: { section: GeneralSection }) {
  const demo = useIsDemo()
  const writable = useCapability('settings_write') === true
  const viewerReason = useReadOnly()
  const loaded = useAsync(() => (demo || !writable ? Promise.resolve(null) : getSettings()), [demo, writable])
  const [draft, setDraft] = useState<Record<string, string>>({})
  const [transcode, setTranscode] = useState<boolean | null>(null)
  const [hwaccel, setHwaccel] = useState<'auto' | 'off' | null>(null)
  const hwState = useLive((st) => st.system?.hwaccel)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [saved, setSaved] = useState<GeneralSettings | null>(null)

  if (demo) return <p className="px-5 py-4 text-sm text-muted">{t('Non modifiable en mode démo.', 'Cannot be changed in demo mode.')}</p>
  if (!writable) return <p className="px-5 py-4 text-sm text-muted">{t("Ce serveur ne permet pas de modifier ces réglages depuis l'interface.", "This server does not allow changing these settings from the interface.")}</p>
  if (loaded.error) return <ErrorState error={loaded.error} onRetry={loaded.reload} compact />
  const s = saved ?? loaded.data
  if (!s) return <LoadingState compact />

  const fields = FIELDS[section]
  const value = (f: FieldDef) => draft[f.key] ?? String(read(s, f.key))
  const invalid = fields.find((f) => {
    const n = Number(value(f))
    return !Number.isInteger(n) || n < f.min || n > f.max
  })
  const currentHw = s.live_hwaccel ?? 'auto'
  const changed =
    fields.some((f) => value(f) !== String(read(s, f.key))) || (transcode !== null && transcode !== s.live_transcode) || (hwaccel !== null && hwaccel !== currentHw)

  const save = async () => {
    setBusy(true)
    setError(null)
    try {
      let next = s
      for (const f of fields) next = write(next, f.key, Number(value(f)))
      const result = await setSettings({
        recording: next.recording,
        motion: next.motion,
        live_transcode: transcode ?? s.live_transcode,
        live_max_bitrate_kbps: next.live_max_bitrate_kbps,
        // Absent: an older server keeps its own (it would refuse the field).
        ...(s.live_hwaccel !== undefined ? { live_hwaccel: hwaccel ?? currentHw } : {}),
        low_space_percent: next.low_space_percent,
      })
      setSaved(result)
      setDraft({})
      setTranscode(null)
      setHwaccel(null)
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-3 px-5 py-4">
      {section === 'live' && (
        <label className="flex items-start gap-2 text-sm">
          <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={transcode ?? s.live_transcode} onChange={(e) => setTranscode(e.target.checked)} />
          <span>
            {t('Convertir le direct en H.264 pour les navigateurs qui ne lisent pas le H.265', 'Convert the live view to H.264 for browsers that cannot play H.265')}
            <span className="block text-xs text-muted">
              {t('Par FFmpeg (', 'By FFmpeg (')}<code>{s.ffmpeg}</code>{t("), seulement pendant qu'on regarde.", "), only while someone watches.")}
            </span>
          </span>
        </label>
      )}
      {section === 'live' && s.live_hwaccel !== undefined && (
        <div className="space-y-1.5 text-sm">
          <label className="flex items-start gap-2">
            <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={(hwaccel ?? currentHw) === 'auto'} onChange={(e) => setHwaccel(e.target.checked ? 'auto' : 'off')} />
            <span>
              {t('Convertir sur la carte graphique quand c’est possible (accélération matérielle)', 'Convert on the graphics card when possible (hardware acceleration)')}
              <span className="block text-xs text-muted">
                {t('Direct, relecture et exports convertis en H.264, par NVIDIA (NVENC), Intel Quick Sync ou AMD (VAAPI). Un essai est fait au démarrage ; sans carte utilisable, ou si l’essai échoue, le processeur s’en charge comme avant.', 'Live view, playback and exports converted to H.264, by NVIDIA (NVENC), Intel Quick Sync or AMD (VAAPI). A test runs at startup; without a usable card, or if the test fails, the processor does it as before.')}{' '}
                <ConfigKey name="live.hwaccel" />
              </span>
            </span>
          </label>
          {hwState && <HwStatus hw={hwState} />}
        </div>
      )}
      <div className="grid gap-3 sm:grid-cols-2">
        {fields.map((f) => (
          <label key={f.key} className="text-xs font-medium text-muted">
            {f.label} ({f.unit})
            <input
              className="mt-1 h-9 w-full rounded-md border border-line bg-bg-2 px-3 text-sm text-fg outline-none focus:border-accent"
              inputMode="numeric"
              value={value(f)}
              onChange={(e) => {
                setDraft((d) => ({ ...d, [f.key]: e.target.value }))
                setError(null)
              }}
            />
            <span className="mt-1 block font-normal">
              {f.help} {f.min}–{f.max}. <ConfigKey name={f.key.includes('.') ? f.key : f.key === 'low_space_percent' ? 'storage.low_space_percent' : 'live.max_bitrate_kbps'} />
            </span>
          </label>
        ))}
      </div>
      {error && <p className="rounded-md border border-err/40 bg-err/10 px-3 py-2 text-sm text-err">{error}</p>}
      <div className="flex flex-wrap items-center gap-3">
        <Button
          variant="primary"
          icon={<Save className="size-4" />}
          disabledReason={viewerReason ? viewerReason : invalid ? t(`${invalid.label} : entre ${invalid.min} et ${invalid.max}.`, `${invalid.label}: between ${invalid.min} and ${invalid.max}.`) : !changed ? t('Aucun changement.', 'No change.') : busy ? t('Application…', 'Applying…') : null}
          onClick={() => void save()}
        >
          {t('Enregistrer', 'Save')}
        </Button>
        <span className="text-xs text-muted">{NOTE[section]}</span>
      </div>
      {s.managed && <p className="text-xs text-muted">{t("Ces réglages viennent de l'interface (settings.ui.toml) et remplacent ceux de config.toml.", "These settings come from the interface (settings.ui.toml) and replace those of config.toml.")}</p>}
    </div>
  )
}
