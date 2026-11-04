import { CircleCheck, CircleMinus } from 'lucide-react'
import { Logo } from '../components/layout/Logo'
import { Card, CardHeader, PageHeader } from '../components/ui/Card'
import { LoadingState } from '../components/ui/States'
import { UpdatesCard } from '../components/layout/UpdatesCard'
import { useData, useLive } from '../data/DataContext'
import type { CapabilityKey } from '../types/api'
import { t } from '../lib/i18n'

const FEATURES: [CapabilityKey, string][] = [
  ['recordings', t('Enregistrement continu et relecture', 'Continuous recording and playback')],
  ['live_video', t('Direct vidéo (MSE)', 'Live video (MSE)')],
  ['live_webrtc', t('Direct faible latence (WebRTC, expérimental)', 'Low-latency live view (WebRTC, experimental)')],
  ['snapshots', t('Captures instantanées', 'Snapshots')],
  ['events', t('Événements', 'Events')],
  ['motion_detection', t('Détection de mouvement', 'Motion detection')],
  ['camera_management', t('Gestion des caméras depuis l’interface', 'Camera management from the interface')],
  ['camera_probe', t('Test de connexion depuis l’interface', 'Connection test from the interface')],
  ['retention', t('Rétention et quotas', 'Retention and quotas')],
  ['settings_write', t('Modification des paramètres', 'Changing settings')],
  ['mqtt', 'MQTT / Home Assistant'],
  ['users', t('Gestion des utilisateurs', 'User management')],
  ['ai', t('IA optionnelle', 'Optional AI')],
]

export function AboutPage() {
  const { capabilities, capabilitiesError, mode } = useData()
  const system = useLive((s) => s.system)
  return (
    <>
      <PageHeader title={t('À propos', 'About')} />
      <div className="grid gap-4 lg:grid-cols-2">
        <Card className="space-y-4 p-5">
          <Logo />
          <p className="text-sm text-muted">
            {t('Enregistreur vidéo réseau open source écrit en Rust : flux RTSP H.264/H.265 enregistrés sans réencodage, faible consommation, IA optionnelle désactivée par défaut.', 'Open source network video recorder written in Rust: RTSP H.264/H.265 streams recorded without re-encoding, low resource use, optional AI off by default.')}
          </p>
          <dl className="grid grid-cols-2 gap-2 text-sm">
            <dt className="text-muted">{t('Version du serveur', 'Server version')}</dt>
            <dd>{mode === 'demo' ? t('mode démo', 'demo mode') : (system?.version ?? '—')}</dd>
            <dt className="text-muted">API</dt>
            <dd>v{capabilities?.api_version ?? '—'}</dd>
            <dt className="text-muted">{t('Licence', 'Licence')}</dt>
            <dd>MIT OR Apache-2.0</dd>
            <dt className="text-muted">{t('Interface', 'Interface')}</dt>
            <dd>React, TypeScript, Tailwind CSS, Lucide</dd>
          </dl>
        </Card>
        <Card>
          <CardHeader title={t('Fonctionnalités du serveur', 'Server features')} subtitle={t('Annoncées par GET /api/v1/capabilities', 'Announced by GET /api/v1/capabilities')} />
          {!capabilities ? (
            <LoadingState compact label={capabilitiesError ? t(`Serveur injoignable : ${capabilitiesError}`, `Server unreachable: ${capabilitiesError}`) : t('Chargement…', 'Loading…')} />
          ) : (
            <ul className="divide-y divide-line">
              {FEATURES.map(([key, label]) => (
                <li key={key} className="flex items-center justify-between px-4 py-2 text-sm">
                  <span>{label}</span>
                  {capabilities[key] ? (
                    <span className="flex items-center gap-1 text-ok">
                      <CircleCheck className="size-4" aria-hidden />{t(' Disponible', ' Available')}
                    </span>
                  ) : (
                    <span className="flex items-center gap-1 text-muted">
                      <CircleMinus className="size-4" aria-hidden />{t(' Pas encore', ' Not yet')}
                    </span>
                  )}
                </li>
              ))}
            </ul>
          )}
        </Card>
        <UpdatesCard />
      </div>
    </>
  )
}
