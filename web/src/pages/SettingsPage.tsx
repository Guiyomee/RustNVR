import { useState, type ReactNode } from 'react'
import { ConfigKey, SettingRow, SettingsSection } from '../components/settings/SettingsSection'
import { AiSettings } from '../components/settings/AiSettings'
import { BackupSettings } from '../components/settings/BackupSettings'
import { GeneralSettingsForm } from '../components/settings/GeneralSettingsForm'
import { LogsViewer } from '../components/settings/LogsViewer'
import { MqttSettings } from '../components/settings/MqttSettings'
import { StorageSettings } from '../components/settings/StorageSettings'
import { UsersSettings } from '../components/settings/UsersSettings'
import { WebhooksSettings } from '../components/settings/WebhooksSettings'
import { Button } from '../components/ui/Button'
import { PageHeader } from '../components/ui/Card'
import { useData, useLive } from '../data/DataContext'
import { formatBytes, formatDuration, formatNumber, formatPercent } from '../lib/format'
import type { SystemInfo } from '../types/api'
import { useLocation } from '../lib/router'
import { useTheme } from '../lib/theme'
import { Segmented } from '../components/ui/Tabs'
import { lang, setLang, t } from '../lib/i18n'

type Category =
    | 'backup'
| 'mqtt'
  | 'webhooks'
| 'general'
  | 'cameras'
  | 'recording'
  | 'motion'
  | 'ai'
  | 'storage'
  | 'mqtt'
  | 'network'
  | 'users'
  | 'logs'
  | 'system'

const CATEGORIES: { key: Category; label: string }[] = [
  { key: 'general', label: t('Général', 'General') },
  { key: 'cameras', label: t('Caméras', 'Cameras') },
  { key: 'recording', label: t('Enregistrement', 'Recording') },
  { key: 'motion', label: t('Détection de mouvement', 'Motion detection') },
  { key: 'ai', label: t('Intelligence artificielle', 'Artificial intelligence') },
  { key: 'storage', label: t('Stockage', 'Storage') },
  { key: 'mqtt', label: 'MQTT / Home Assistant' },
  { key: 'webhooks', label: t('Webhooks', 'Webhooks') },
  { key: 'backup', label: t('Sauvegarde', 'Backup') },
  { key: 'network', label: t('Réseau', 'Network') },
  { key: 'users', label: t('Utilisateurs', 'Users') },
  { key: 'logs', label: t('Journaux', 'Logs') },
  { key: 'system', label: t('Système', 'System') },
]

function ThemeSetting() {
  const [theme, setTheme] = useTheme()
  return (
    <Segmented
      label={t('Thème', 'Theme')}
      options={[
        { value: 'dark' as const, label: t('Sombre', 'Dark') },
        { value: 'light' as const, label: t('Clair', 'Light') },
        { value: 'system' as const, label: t('Système', 'System') },
      ]}
      value={theme}
      onChange={setTheme}
    />
  )
}

const FILE_NOTE = t('Ces réglages se modifient dans config.toml (redémarrage requis).', 'These settings are changed in config.toml (restart required).')

function Section({ category }: { category: Category }) {
  const { mode, setMode, capabilities } = useData()
  const system = useLive((s) => s.system)
  const cameras = useLive((s) => s.cameras)
  const cap = (k: keyof NonNullable<typeof capabilities>) => (capabilities ? Boolean(capabilities[k]) : null)
  const yes = (v: boolean | null | undefined): ReactNode => (v == null ? '—' : v ? t('Oui', 'Yes') : t('Non', 'No'))

  switch (category) {
    case 'general':
      return (
        <SettingsSection title={t('Général', 'General')} description={t('Préférences de cette interface, enregistrées dans le navigateur.', 'Preferences of this interface, saved in the browser.')}>
          <SettingRow label={t('Mode démo', 'Demo mode')} help={t('Affiche des caméras, images et événements fictifs, sans jamais les mélanger aux données réelles.', 'Shows fictional cameras, pictures and events, never mixed with real data.')}>
            <Button variant={mode === 'demo' ? 'primary' : 'secondary'} onClick={() => setMode(mode === 'demo' ? 'live' : 'demo')} aria-pressed={mode === 'demo'}>
              {mode === 'demo' ? t('Désactiver le mode démo', 'Turn demo mode off') : t('Activer le mode démo', 'Turn demo mode on')}
            </Button>
          </SettingRow>
          <SettingRow label={t('Thème', 'Theme')} help={t('Retenu par ce navigateur. « Système » suit le réglage clair ou sombre de l’appareil.', 'Kept by this browser. « System » follows the light or dark setting of the device.')}>
            <ThemeSetting />
          </SettingRow>
          <SettingRow label={t('Langue', 'Language')} help={t('Retenue par ce navigateur ; la page se recharge. Les messages venant du serveur (erreurs, journal) restent en français.', 'Kept by this browser; the page reloads. Messages coming from the server (errors, log) stay in French.')}>
            <Segmented
              label={t('Langue', 'Language')}
              options={[
                { value: 'fr' as const, label: 'Français' },
                { value: 'en' as const, label: 'English' },
              ]}
              value={lang()}
              onChange={setLang}
            />
          </SettingRow>
        </SettingsSection>
      )
    case 'cameras':
      return (
        <SettingsSection title={t('Caméras', 'Cameras')} description={FILE_NOTE} available={cap('camera_management')}>
          <SettingRow label={t('Caméras configurées', 'Configured cameras')} help={<ConfigKey name="[[cameras]]" />}>
            {formatNumber(cameras.length)}
          </SettingRow>
          <SettingRow label={t('Délai de connexion', 'Connection timeout')} help={<ConfigKey name="main.connect_timeout_seconds" />} advanced>
            {t('10 s par défaut', '10 s by default')}
          </SettingRow>
          <SettingRow label={t('Détection de flux bloqué', 'Stalled stream detection')} help={<ConfigKey name="main.stall_timeout_seconds" />} advanced>
            {t('10 s par défaut', '10 s by default')}
          </SettingRow>
          <SettingRow label={t('Reconnexion', 'Reconnection')} help={<ConfigKey name="[reconnect]" />} advanced>
            {t('1 s → 30 s, exponentielle', '1 s → 30 s, exponential')}
          </SettingRow>
        </SettingsSection>
      )
    case 'recording':
      return (
        <SettingsSection title={t('Enregistrement', 'Recording')} description={t('Sans réencodage, en MP4 fragmenté. Le mode (continu ou sur mouvement) se choisit dans la fiche de chaque caméra.', 'No re-encoding, in fragmented MP4. The mode (continuous or on motion) is chosen in the settings of each camera.')} available={cap('recordings')}>
          <GeneralSettingsForm section="recording" />
        </SettingsSection>
      )
    case 'motion':
      return (
        <SettingsSection
          title={t('Détection de mouvement', 'Motion detection')}
          description={t("Moteur léger sans IA : FFmpeg décode le sous-flux en petites images grises, comparées à un fond qui s'adapte lentement. Elle s'active caméra par caméra (onglet Détection de chaque caméra).", "Light engine without AI: FFmpeg decodes the sub stream into small grey pictures, compared with a slowly adapting background. It is turned on camera by camera (Detection tab of each camera).")}
          available={cap('motion_detection')}
        >
          <SettingRow label={t('Caméras avec détection', 'Cameras with detection')}>
            {formatNumber(cameras.filter((c) => c.motion.enabled).length)} / {formatNumber(cameras.length)}
          </SettingRow>
          <SettingRow label={t('Mouvement en cours', 'Motion now')}>
            {cameras.filter((c) => c.motion.active).map((c) => c.name).join(', ') || 'aucun'}
          </SettingRow>
          <GeneralSettingsForm section="motion" />
        </SettingsSection>
      )
    case 'ai':
      return (
        <SettingsSection
          title={t('Intelligence artificielle', 'Artificial intelligence')}
          description={t("Optionnelle : reconnaît les objets (personnes, véhicules, animaux…) sur les images des mouvements détectés. Aucun modèle n'est fourni ; RustNVR fonctionne entièrement sans IA.", "Optional: recognizes objects (people, vehicles, animals…) in the pictures of detected motion. No model is shipped; RustNVR works fully without AI.")}
          available={cap('settings_write')}
        >
          <AiSettings />
        </SettingsSection>
      )
    case 'storage':
      return (
        <SettingsSection
          title={t('Stockage', 'Storage')}
          description={t("Points de stockage (disque local, partages NAS montés, S3, SFTP) et rétention des vidéos. Les modifications s'appliquent immédiatement, sans redémarrage.", "Storage destinations (local disk, mounted NAS shares, S3, SFTP) and video retention. Changes apply at once, without restart.")}
          available={cap('retention')}
        >
          <StorageSettings />
          <GeneralSettingsForm section="storage" />
        </SettingsSection>
      )
    case 'mqtt':
      return (
        <SettingsSection
          title="MQTT / Home Assistant"
          description={t("Publie l'état des caméras, les mouvements et leurs images, avec découverte automatique dans Home Assistant. Appliqué immédiatement.", "Publishes the state of the cameras, motion and its pictures, with automatic discovery in Home Assistant. Applied at once.")}
          available={cap('mqtt')}
        >
          <MqttSettings />
        </SettingsSection>
      )
    case 'network':
      return (
        <SettingsSection title={t('Réseau', 'Network')} description={FILE_NOTE}>
          <SettingRow label={t("Adresse de l'interface", "Interface address")} help={<ConfigKey name="web.listen" />}>
            <code>{window.location.host}</code>
          </SettingRow>
          <SettingRow label={t('Connexion', 'Sign in')} help={<>{t('HTTPS intégré : ', 'Built-in HTTPS: ')}<ConfigKey name="web.tls_self_signed" />{t(' ou ', ' or ')}<ConfigKey name="web.tls_cert" /> + <ConfigKey name="web.tls_key" />{t(' (redémarrage requis).', ' (restart required).')}</>}>
            {window.location.protocol === 'https:' ? 'HTTPS' : t('HTTP (non chiffré)', 'HTTP (not encrypted)')}
          </SettingRow>
          <GeneralSettingsForm section="live" />
        </SettingsSection>
      )
    case 'webhooks':
      return (
        <SettingsSection title={t('Webhooks', 'Webhooks')} description={t('Adresses appelées à chaque événement (mouvement, objets reconnus, fin), avec le JSON de l’événement et son image : n8n, Node-RED, Home Assistant, script… sans MQTT. Appliqués tout de suite.', 'Addresses called on each event (motion, recognized objects, end), with the JSON of the event and its picture: n8n, Node-RED, Home Assistant, script… without MQTT. Applied at once.')} available={cap('settings_write')}>
          <WebhooksSettings />
        </SettingsSection>
      )
    case 'backup':
      return (
        <SettingsSection title={t('Sauvegarde de la configuration', 'Configuration backup')} description={t('Avant une réinstallation ou un changement de machine : un fichier chiffré par une phrase secrète, à restaurer ici ensuite. La restauration prend effet au redémarrage de RustNVR.', 'Before reinstalling or moving to another machine: a file encrypted with a passphrase, to restore here afterwards. A restore takes effect when RustNVR restarts.')} available={cap('backup')}>
          <BackupSettings />
        </SettingsSection>
      )
    case 'logs':
      return (
        <SettingsSection title={t('Journaux', 'Logs')} description={t('Derniers messages de RustNVR, mis à jour en direct : connexions des caméras, enregistrement, détection, erreurs.', 'Latest messages of RustNVR, updated live: camera connections, recording, detection, errors.')}>
          <LogsViewer />
        </SettingsSection>
      )
    case 'users':
      return (
        <SettingsSection
          title={t('Utilisateurs', 'Users')}
          description={t("Comptes de l'interface : administrateurs (tout) et lecteurs (consultation). Mots de passe chiffrés (Argon2), sessions de 30 jours, blocage après 5 essais manqués.", "Interface accounts: administrators (everything) and viewers (read only). Hashed passwords (Argon2), 30-day sessions, blocked after 5 failed attempts.")}
          available={cap('users')}
        >
          <UsersSettings />
        </SettingsSection>
      )
    case 'system':
      return (
        <SettingsSection title={t('Système', 'System')}>
          <SettingRow label={t('Version', 'Version')}>{system?.version ?? '—'}</SettingRow>
          <SettingRow label={t('Démarré depuis', 'Running for')}>{system ? formatDuration(system.uptime_seconds * 1000) : '—'}</SettingRow>
          <SettingRow label={t('Mémoire de RustNVR (avec FFmpeg)', 'RustNVR memory (with FFmpeg)')} help={scopeHelp(system?.service_scope)}>
            {system?.service_memory_bytes == null
              ? '—'
              : system.service_memory_limit_bytes
                ? t(`${formatBytes(system.service_memory_bytes)} sur ${formatBytes(system.service_memory_limit_bytes)} autorisés`, `${formatBytes(system.service_memory_bytes)} out of ${formatBytes(system.service_memory_limit_bytes)} allowed`)
                : formatBytes(system.service_memory_bytes)}
          </SettingRow>
          <SettingRow label={t('CPU de RustNVR (avec FFmpeg)', 'RustNVR CPU (with FFmpeg)')} help={t("En pourcentage d'un cœur, comme docker stats : 200 % = deux cœurs pleins.", 'As a percentage of one core, like docker stats: 200 % = two full cores.')}>
            {system?.service_cpu_percent == null ? '—' : `${formatNumber(system.service_cpu_percent, 1)} %`}
          </SettingRow>
          <SettingRow label={t('Programme RustNVR seul', 'RustNVR program alone')} help={t("Sans les FFmpeg qu'il lance : mémoire résidente et CPU en % d'un cœur.", 'Without the FFmpeg processes it starts: resident memory and CPU as % of one core.')}>
            {`${formatBytes(system?.process_rss_bytes)} · ${system?.process_cpu_percent == null ? '—' : `${formatNumber(system.process_cpu_percent, 1)} %`}`}
          </SettingRow>
          <SettingRow label={t('Machine', 'Machine')} help={t('Toute la machine (sous Docker : la machine hôte), CPU sur tous les cœurs ; affichée en haut de page.', 'The whole machine (under Docker: the host), CPU over all cores; shown at the top of the page.')}>
            {`CPU ${formatPercent(system?.cpu_percent)} · RAM ${formatBytes(system?.memory_used_bytes)} / ${formatBytes(system?.memory_total_bytes)}`}
          </SettingRow>
          <SettingRow label={t('Direct vidéo disponible', 'Live video available')}>{yes(cap('live_video'))}</SettingRow>
        </SettingsSection>
      )
  }
}

/** What the « with FFmpeg » figures cover, for the help text. */
function scopeHelp(scope: SystemInfo['service_scope']): string {
  switch (scope) {
    case 'container':
      return t('Tout le conteneur : même calcul que docker stats (cache de fichiers actif compris).', 'The whole container: same figure as docker stats (active file cache included).')
    case 'service':
      return t('Tout le service systemd : RustNVR et les FFmpeg qu’il lance (cache de fichiers actif compris).', 'The whole systemd service: RustNVR and the FFmpeg processes it starts (active file cache included).')
    case 'processes':
      return t('RustNVR et les FFmpeg qu’il lance (somme des mémoires résidentes).', 'RustNVR and the FFmpeg processes it starts (resident memory added up).')
    default:
      return t('Mesure disponible sous Linux seulement.', 'Measured on Linux only.')
  }
}

export function SettingsPage() {
  const requested = useLocation().query.get('section')
  const [category, setCategory] = useState<Category>(CATEGORIES.find((c) => c.key === requested)?.key ?? 'general')
  return (
    <>
      <PageHeader title={t('Paramètres', 'Settings')} />
      <div className="grid gap-4 lg:grid-cols-[220px_minmax(0,1fr)]">
        <nav aria-label={t('Catégories de paramètres', 'Settings categories')} className="flex gap-1 overflow-x-auto lg:flex-col">
          {CATEGORIES.map((c) => (
            <button
              key={c.key}
              type="button"
              onClick={() => setCategory(c.key)}
              aria-current={c.key === category ? 'true' : undefined}
              className={`shrink-0 rounded-md px-3 py-2 text-left text-sm ${c.key === category ? 'bg-card text-fg' : 'text-muted hover:text-fg'}`}
            >
              {c.label}
            </button>
          ))}
        </nav>
        <Section category={category} />
      </div>
    </>
  )
}
