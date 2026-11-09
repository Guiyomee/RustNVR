import { useLive } from '../data/DataContext'
import type { Tone } from '../components/ui/StatusIndicator'
import { t } from '../lib/i18n'

export interface GlobalStatus {
  tone: Tone
  label: string
  detail: string
}

/** Overall NVR state shown in the top bar, derived from real live data only. */
export function useGlobalStatus(): GlobalStatus {
  const connection = useLive((s) => s.connection)
  const error = useLive((s) => s.error)
  const authRequired = useLive((s) => s.authRequired)
  const cameras = useLive((s) => s.cameras)
  const loaded = useLive((s) => s.loaded)

  if (authRequired) return { tone: 'warn', label: t('Authentification requise', 'Sign-in required'), detail: t('Rechargez la page pour vous identifier.', 'Reload the page to sign in.') }
  if (error && connection !== 'open') return { tone: 'err', label: t('Connexion perdue', 'Connection lost'), detail: error }
  if (!loaded) return { tone: 'neutral', label: t('Connexion…', 'Connecting…'), detail: t('Connexion au serveur RustNVR', 'Connecting to the RustNVR server') }
  const enabled = cameras.filter((c) => c.enabled)
  const online = enabled.filter((c) => c.status === 'online').length
  const writeErrors = enabled.some((c) => (c.recorder?.write_errors ?? 0) > 0)
  if (enabled.length === 0) return { tone: 'neutral', label: t('Aucune caméra', 'No camera'), detail: t('Aucune caméra activée dans la configuration.', 'No camera on in the configuration.') }
  if (online === 0) return { tone: 'err', label: t('Aucune caméra en ligne', 'No camera online'), detail: t(`${enabled.length} caméra(s) hors ligne`, `${enabled.length} camera(s) offline`) }
  if (online < enabled.length) return { tone: 'warn', label: t('Dégradé', 'Degraded'), detail: t(`${enabled.length - online} caméra(s) hors ligne`, `${enabled.length - online} camera(s) offline`) }
  if (writeErrors) return { tone: 'warn', label: t('Dégradé', 'Degraded'), detail: t("Des erreurs d'écriture ont eu lieu depuis le démarrage.", "Write errors happened since startup.") }
  if (enabled.some((c) => c.motion.paused)) return { tone: 'warn', label: t('Détection en pause', 'Detection paused'), detail: t('La détection de mouvement est en pause sur toutes les caméras.', 'Motion detection is paused on every camera.') }
  if (connection === 'polling') return { tone: 'warn', label: t('Opérationnel', 'Operational'), detail: t('Temps réel interrompu : actualisation toutes les 10 s.', 'Real time interrupted: refreshed every 10 s.') }
  return { tone: 'ok', label: t('Opérationnel', 'Operational'), detail: t('Toutes les caméras activées sont en ligne.', 'All cameras that are on are online.') }
}
