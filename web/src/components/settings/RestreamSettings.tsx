import { Save } from 'lucide-react'
import { useState } from 'react'
import { describeError } from '../../api/client'
import { useReadOnly } from '../../auth/AuthGate'
import { getSettings, setSettings } from '../../api/endpoints'
import { useCapability, useData, useIsDemo } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { t } from '../../lib/i18n'
import { restreamHost } from '../../lib/restream'
import type { GeneralSettings, RestreamSettings as Restream } from '../../types/api'
import { Button } from '../ui/Button'
import { ErrorState, LoadingState } from '../ui/States'
import { ConfigKey } from './SettingsSection'

const DEFAULT_PORT = 8554

/** Port of a listening address (`127.0.0.1:8554`, `[::]:8554`), or null. */
function portOf(listen: string): number | null {
  const m = /:(\d{1,5})$/.exec(listen.trim())
  const port = m ? Number(m[1]) : NaN
  return Number.isInteger(port) && port >= 1 && port <= 65535 ? port : null
}

/** Whether the address only listens on the server itself. */
function isLocal(listen: string): boolean {
  const host = listen.trim().replace(/:\d+$/, '')
  return host === '127.0.0.1' || host === '[::1]' || host.startsWith('127.')
}

/** RTSP republication of the cameras (`[restream]`, saved in settings.ui.toml). */
export function RestreamSettings() {
  const demo = useIsDemo()
  const writable = useCapability('settings_write') === true
  const viewerReason = useReadOnly()
  const { reloadCapabilities } = useData()
  const loaded = useAsync(() => (demo || !writable ? Promise.resolve(null) : getSettings()), [demo, writable])
  const [saved, setSaved] = useState<GeneralSettings | null>(null)
  const [draft, setDraft] = useState<Restream | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  if (demo) return <p className="px-5 py-4 text-sm text-muted">{t('Non modifiable en mode démo.', 'Cannot be changed in demo mode.')}</p>
  if (!writable) return <p className="px-5 py-4 text-sm text-muted">{t("Ce serveur ne permet pas de modifier ces réglages depuis l'interface.", "This server does not allow changing these settings from the interface.")}</p>
  if (loaded.error) return <ErrorState error={loaded.error} onRetry={loaded.reload} compact />
  const s = saved ?? loaded.data
  if (!s) return <LoadingState compact />
  const current = s.restream
  if (!current) return <p className="px-5 py-4 text-sm text-muted">{t('Ce serveur ne sait pas republier les flux en RTSP.', 'This server cannot republish the streams over RTSP.')}</p>

  const r = draft ?? current
  const edit = (patch: Partial<Restream>) => {
    setDraft({ ...r, ...patch })
    setError(null)
  }
  const local = isLocal(r.listen)
  const port = portOf(r.listen)
  const changed = draft !== null && JSON.stringify(draft) !== JSON.stringify(current)
  const invalid =
    port === null
      ? t("Adresse d'écoute : de la forme 127.0.0.1:8554 ou 0.0.0.0:8554.", 'Listening address: like 127.0.0.1:8554 or 0.0.0.0:8554.')
      : !Number.isInteger(r.max_clients) || r.max_clients < 1 || r.max_clients > 256
        ? t('Clients simultanés : entre 1 et 256.', 'Simultaneous clients: between 1 and 256.')
        : r.enabled && !local && !r.allow_unauthenticated_remote
          ? t('Cochez la case d’accès sans mot de passe pour écouter sur le réseau.', 'Tick the box about access without a password to listen on the network.')
          : null

  const save = async () => {
    setBusy(true)
    setError(null)
    try {
      // Fresh settings, so that changes made in other sections are kept.
      const now = await getSettings()
      const result = await setSettings({
        recording: now.recording,
        motion: now.motion,
        live_transcode: now.live_transcode,
        live_max_bitrate_kbps: now.live_max_bitrate_kbps,
        low_space_percent: now.low_space_percent,
        // The agreement is only kept while it is needed.
        restream: { ...r, listen: r.listen.trim(), allow_unauthenticated_remote: r.allow_unauthenticated_remote && !local },
      })
      setSaved(result)
      setDraft(null)
      // The camera pages show the addresses.
      reloadCapabilities()
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }

  const served = current.enabled && !s.restream_error
  const servedPort = portOf(current.listen) ?? DEFAULT_PORT
  const base = `rtsp://${restreamHost(isLocal(current.listen))}:${servedPort}`
  const input = 'mt-1 h-9 w-full rounded-md border border-line bg-bg-2 px-3 text-sm text-fg outline-none focus:border-accent'

  return (
    <div className="space-y-3 px-5 py-4">
      <div>
        <h3 className="text-sm font-semibold">{t('Republication RTSP', 'RTSP republication')}</h3>
        <p className="mt-1 text-xs text-muted">
          {t(
            'Home Assistant, Frigate, VLC… lisent les caméras depuis RustNVR au lieu de s’y connecter eux-mêmes : la caméra ne voit qu’une connexion. Les images sont renvoyées telles quelles (H.264 / H.265, son AAC ou Opus), sans conversion : presque aucun processeur, et rien du tout quand personne ne regarde.',
            'Home Assistant, Frigate, VLC… read the cameras from RustNVR instead of connecting to them: the camera sees a single connection. Pictures are sent as they are (H.264 / H.265, AAC or Opus sound), without conversion: almost no processor, and nothing at all while nobody watches.',
          )}{' '}
          <ConfigKey name="[restream]" />
        </p>
      </div>
      <label className="flex items-start gap-2 text-sm">
        <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={r.enabled} onChange={(e) => edit({ enabled: e.target.checked })} />
        <span>
          {t('Republier les flux des caméras en RTSP', 'Republish the camera streams over RTSP')}
          <span className="block text-xs text-muted">{t('Sans mot de passe ; RTP dans la connexion RTSP (TCP) uniquement.', 'No password; RTP inside the RTSP connection (TCP) only.')}</span>
        </span>
      </label>
      <div className="grid gap-3 sm:grid-cols-2">
        <label className="text-xs font-medium text-muted">
          {t("Adresse d'écoute", 'Listening address')}
          <input className={input} value={r.listen} spellCheck={false} onChange={(e) => edit({ listen: e.target.value })} />
          <span className="mt-1 block font-normal">
            {t('127.0.0.1:8554 : cette machine seulement ; 0.0.0.0:8554 : tout le réseau.', '127.0.0.1:8554: this machine only; 0.0.0.0:8554: the whole network.')} <ConfigKey name="restream.listen" />
          </span>
        </label>
        <label className="text-xs font-medium text-muted">
          {t('Clients simultanés (max.)', 'Simultaneous clients (max.)')}
          <input className={input} inputMode="numeric" value={Number.isNaN(r.max_clients) ? '' : String(r.max_clients)} onChange={(e) => edit({ max_clients: e.target.value === '' ? NaN : Number(e.target.value) })} />
          <span className="mt-1 block font-normal">
            {t('Chaque client reçoit le débit de la caméra. 1–256.', 'Each client receives the bit rate of the camera. 1–256.')} <ConfigKey name="restream.max_clients" />
          </span>
        </label>
      </div>
      {!local && port !== null && (
        <label className="flex items-start gap-2 rounded-md border border-warn/40 bg-warn/10 px-3 py-2 text-sm">
          <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={r.allow_unauthenticated_remote} onChange={(e) => edit({ allow_unauthenticated_remote: e.target.checked })} />
          <span>
            {t('J’accepte que toute machine du réseau qui joint cette adresse voie les caméras sans mot de passe', 'I accept that any machine on the network reaching this address watches the cameras without a password')}
            <span className="block text-xs text-muted">
              {t('À réserver à un réseau local de confiance ; ne pas exposer ce port sur Internet.', 'Keep it to a trusted local network; do not expose this port to the Internet.')} <ConfigKey name="restream.allow_unauthenticated_remote" />
            </span>
          </span>
        </label>
      )}
      {s.restream_error && current.enabled && <p className="rounded-md border border-err/40 bg-err/10 px-3 py-2 text-sm text-err">{t('La republication n’a pas démarré : ', 'The republication did not start: ')}{s.restream_error}</p>}
      {served && (
        <p className="text-xs text-muted">
          {t('Active. Adresses : ', 'Running. Addresses: ')}
          <code>{base}/&lt;{t('identifiant de la caméra', 'camera id')}&gt;</code>
          {t(' (flux principal) et ', ' (main stream) and ')}
          <code>{base}/&lt;{t('identifiant', 'id')}&gt;/sub</code>
          {t(' (sous-flux) ; elles figurent sur la page de chaque caméra.', ' (sub stream); they are shown on the page of each camera.')}
          {isLocal(current.listen) && t(' Utilisables depuis cette machine seulement.', ' Usable from this machine only.')}
        </p>
      )}
      {error && <p className="rounded-md border border-err/40 bg-err/10 px-3 py-2 text-sm text-err">{error}</p>}
      <div className="flex flex-wrap items-center gap-3">
        <Button
          variant="primary"
          icon={<Save className="size-4" />}
          disabledReason={viewerReason ? viewerReason : invalid ? invalid : !changed ? t('Aucun changement.', 'No change.') : busy ? t('Application…', 'Applying…') : null}
          onClick={() => void save()}
        >
          {t('Enregistrer', 'Save')}
        </Button>
        <span className="text-xs text-muted">{t('Appliqué tout de suite, sans redémarrage ; les clients RTSP connectés sont déconnectés.', 'Applied at once, without restart; connected RTSP clients are disconnected.')}</span>
      </div>
    </div>
  )
}
