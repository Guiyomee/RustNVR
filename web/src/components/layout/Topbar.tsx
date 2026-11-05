import { Bell, CircleUser, Cpu, HardDrive, Gpu, MemoryStick, Menu, ShieldAlert, Video } from 'lucide-react'
import { busiestGpu, computeGpus, gpuStatusLabel } from '../../lib/gpu'
import type { ReactNode } from 'react'
import { useData, useLive } from '../../data/DataContext'
import { useGlobalStatus } from '../../hooks/useSystemStatus'
import { destinationsDefaultFirst, diskPercent, useStorage } from '../../hooks/useStorage'
import { formatBytes, formatNumber, formatPercent, formatTime } from '../../lib/format'
import { useAuth } from '../../auth/AuthGate'
import { navigate } from '../../lib/router'
import { Button, IconButton } from '../ui/Button'
import { Popover } from '../ui/Popover'
import { StatusDot, toneText } from '../ui/StatusIndicator'
import { t } from '../../lib/i18n'

function Metric({ icon, label, value, title, warn = false }: { icon: ReactNode; label: string; value: string; title?: string; warn?: boolean }) {
  return (
    <div className="hidden items-center gap-2 xl:flex" title={title}>
      <span className="text-muted" aria-hidden>
        {icon}
      </span>
      <div className="leading-tight">
        <div className="text-[11px] text-muted">{label}</div>
        <div className={`tabular text-sm font-medium ${warn ? 'text-warn' : ''}`}>{value}</div>
      </div>
    </div>
  )
}

function Notifications() {
  const { source } = useData()
  const notifications = useLive((s) => s.notifications)
  const unread = useLive((s) => s.unread)
  return (
    <Popover
      onOpen={() => source.store.markAllRead()}
      trigger={(_, toggle) => (
        <div className="relative">
          <IconButton label={t(`Notifications${unread ? ` (${unread} non lues)` : ''}`, `Notifications${unread ? ` (${unread} unread)` : ''}`)} icon={<Bell className="size-[18px]" />} onClick={toggle} />
          {unread > 0 && (
            <span className="pointer-events-none absolute -top-0.5 -right-0.5 flex min-w-4 items-center justify-center rounded-full bg-err px-1 text-[10px] font-semibold text-white">
              {unread > 9 ? '9+' : unread}
            </span>
          )}
        </div>
      )}
    >
      <div className="flex items-center justify-between border-b border-line px-4 py-2.5">
        <span className="text-sm font-semibold">{t('Notifications', 'Notifications')}</span>
        {notifications.length > 0 && (
          <button type="button" className="text-xs text-accent hover:underline" onClick={() => source.store.clearNotifications()}>
            {t('Effacer', 'Clear')}
          </button>
        )}
      </div>
      <div className="max-h-80 overflow-y-auto">
        {notifications.length === 0 ? (
          <p className="px-4 py-6 text-center text-sm text-muted">
            {t("Aucune notification depuis l'ouverture de la page.", "No notification since the page was opened.")}
            <br />
            <span className="text-xs">{t("Pertes et retours de flux, erreurs d'écriture.", "Streams lost and back, write errors.")}</span>
          </p>
        ) : (
          <ul className="divide-y divide-line">
            {notifications.map((n) => (
              <li key={n.id} className="flex gap-3 px-4 py-2.5 text-sm">
                <span className={`mt-1.5 size-2 shrink-0 rounded-full ${n.level === 'error' ? 'bg-err' : n.level === 'warning' ? 'bg-warn' : 'bg-accent'}`} aria-hidden />
                <div className="min-w-0">
                  <p className="break-words">{n.message}</p>
                  <p className="text-xs text-muted">{formatTime(n.at, true)}</p>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>
    </Popover>
  )
}

function Profile() {
  const system = useLive((s) => s.system)
  const { user, status, signOut, startSetup } = useAuth()
  const auth = system?.auth_enabled ?? null
  const name = user?.username ?? system?.user
  const role = user ? (user.role === 'admin' ? 'administrateur' : 'lecteur') : null
  return (
    <Popover
      trigger={(_, toggle) => (
        <button type="button" onClick={toggle} className="flex items-center gap-2 rounded-md px-2 py-1.5 hover:bg-card" aria-label={t('Profil utilisateur', 'User profile')}>
          {auth === false ? <ShieldAlert className="size-5 text-warn" aria-hidden /> : <CircleUser className="size-5 text-muted" aria-hidden />}
          <span className="hidden max-w-44 truncate text-sm sm:inline">{auth ? name : auth === false ? t('Sans authentification', 'No authentication') : '…'}</span>
        </button>
      )}
    >
      <div className="space-y-2 px-4 py-3 text-sm">
        {auth ? (
          <>
            <p>
              {t('Connecté en tant que ', 'Signed in as ')}<strong>{name}</strong>
              {role && ` (${role})`}.
            </p>
            {status?.accounts && user?.username ? (
              <div className="flex flex-wrap gap-2">
                <Button size="sm" onClick={() => navigate('/settings?section=users')}>
                  {t('Mon mot de passe', 'My password')}
                </Button>
                <Button size="sm" variant="danger" onClick={() => void signOut()}>
                  {t('Se déconnecter', 'Sign out')}
                </Button>
              </div>
            ) : (
              <p className="text-xs text-muted">{t('Authentification HTTP Basic : fermez le navigateur pour oublier les identifiants.', 'HTTP Basic authentication: close the browser to forget the credentials.')}</p>
            )}
          </>
        ) : (
          <>
            <p className="font-medium text-warn">{t('Accès sans authentification', 'Access without authentication')}</p>
            <p className="text-xs text-muted">{t('Toute personne pouvant joindre cette adresse voit les caméras et les enregistrements.', 'Anyone who can reach this address sees the cameras and the recordings.')}</p>
            {status?.setup && (
              <Button size="sm" variant="primary" onClick={startSetup}>
                {t('Créer le compte administrateur', 'Create the administrator account')}
              </Button>
            )}
          </>
        )}
      </div>
    </Popover>
  )
}

export function Topbar({ onOpenMenu }: { onOpenMenu: () => void }) {
  const status = useGlobalStatus()
  const system = useLive((s) => s.system)
  const { data: storage } = useStorage()
  // RustNVR's own figures (the container under Docker, as `docker stats`),
  // never the whole machine.
  const scope = system?.service_scope
  const who =
    scope === 'container'
      ? t('Conteneur RustNVR (FFmpeg compris), comme docker stats', 'RustNVR container (FFmpeg included), as docker stats')
      : scope === 'service'
        ? t('Service RustNVR (FFmpeg compris)', 'RustNVR service (FFmpeg included)')
        : scope === 'processes'
          ? t('RustNVR et ses FFmpeg', 'RustNVR and its FFmpeg')
          : t('Processus RustNVR', 'RustNVR process')
  const suffix = scope === 'container' ? t(' (conteneur)', ' (container)') : ' (RustNVR)'
  const cpu = system?.service_cpu_percent ?? system?.process_cpu_percent
  const mem = system?.service_memory_bytes ?? system?.process_rss_bytes
  // Out of the container's limit, or of the machine's memory without one (as docker stats).
  const memOf = system?.service_memory_limit_bytes ?? system?.memory_total_bytes
  const memPct = mem != null && memOf ? (100 * mem) / memOf : null
  const gpus = computeGpus(system?.gpus)
  const busiest = busiestGpu(system?.gpus)
  const perDestination = storage
    ? destinationsDefaultFirst(storage)
        .filter((d) => d.recordings_bytes != null)
        .map((d) => {
          const pct = diskPercent(d.disk)
          return `${d.name} : ${formatBytes(d.recordings_bytes)}${pct !== null ? t(` (disque à ${formatNumber(pct)} %)`, ` (disk at ${formatNumber(pct)} %)`) : ''}`
        })
        .join('\n')
    : ''

  return (
    <header className="sticky top-0 z-30 flex h-16 items-center gap-4 border-b border-line bg-bg/95 px-4 backdrop-blur lg:px-6">
      <IconButton className="lg:hidden" label={t('Ouvrir le menu', 'Open the menu')} icon={<Menu className="size-5" />} onClick={onOpenMenu} />

      <div className="flex min-w-0 items-center gap-3" title={status.detail}>
        <StatusDot tone={status.tone} label="" pulse={status.tone === 'ok'} />
        <div className="min-w-0 leading-tight">
          <div className="text-[11px] text-muted">{t('État du NVR', 'NVR state')}</div>
          <div className={`truncate text-sm font-semibold ${toneText[status.tone]}`}>{status.label}</div>
        </div>
      </div>

      <div className="ml-auto flex items-center gap-6">
        <div className="hidden items-center gap-2 md:flex">
          <Video className="size-4 text-muted" aria-hidden />
          <div className="leading-tight">
            <div className="text-[11px] text-muted">{t('Caméras', 'Cameras')}</div>
            <div className="tabular text-sm font-medium">
              {system ? t(`${system.cameras_online} / ${system.cameras_enabled} en ligne`, `${system.cameras_online} / ${system.cameras_enabled} online`) : '—'}
            </div>
          </div>
        </div>
        <Metric
          icon={<Cpu className="size-4" />}
          label={`CPU${suffix}`}
          value={formatPercent(cpu)}
          title={t(`${who} : pourcentage d’un cœur (200 % = deux cœurs occupés).`, `${who}: percent of one core (200 % = two busy cores).`)}
        />
        <Metric
          icon={<MemoryStick className="size-4" />}
          label={`RAM${suffix}`}
          value={mem == null ? '—' : memPct === null ? formatBytes(mem) : `${formatBytes(mem)} · ${formatPercent(memPct)}`}
          title={
            system?.service_memory_limit_bytes != null
              ? t(`${who} : ${formatBytes(mem)} sur la limite de ${formatBytes(system.service_memory_limit_bytes)}.`, `${who}: ${formatBytes(mem)} out of the ${formatBytes(system.service_memory_limit_bytes)} limit.`)
              : t(`${who} : ${formatBytes(mem)}, pourcentage de la mémoire de la machine (${formatBytes(system?.memory_total_bytes)}, aucune limite fixée).`, `${who}: ${formatBytes(mem)}, percent of the machine's memory (${formatBytes(system?.memory_total_bytes)}, no limit set).`)
          }
        />
        {gpus.length > 0 && (
          <Metric
            icon={<Gpu className="size-4" />}
            label="GPU"
            value={formatPercent(busiest?.busy_percent)}
            title={[
              t('Occupation de toute la carte (tous ses utilisateurs, CodeProject.AI compris) :', 'Busy share of the whole card (all its users, CodeProject.AI included):'),
              ...gpus.map((g) => `${g.name} : ${g.busy_percent === null ? t('occupation inconnue', 'busy share unknown') : formatPercent(g.busy_percent)} · ${gpuStatusLabel(g)}`),
            ].join('\n')}
          />
        )}
        <Metric
          icon={<HardDrive className="size-4" />}
          label={t('Vidéos', 'Videos')}
          value={storage ? `${formatBytes(storage.recordings_bytes)}${system?.storage_problems?.length ? t(' · dégradé', ' · degraded') : system?.low_space.length ? t(' · espace faible', ' · low space') : ''}` : '—'}
          warn={Boolean(system?.low_space.length || system?.storage_problems?.length)}
          title={[
            t('Enregistrements de RustNVR, tous points de stockage :', 'RustNVR recordings, every destination:'),
            perDestination,
            ...(system?.storage_problems ?? []).map((p) => t(`Dégradé : ${p.name} — ${p.problem}`, `Degraded: ${p.name} — ${p.problem}`)),
            system?.low_space.length ? t(`Espace faible : ${system.low_space.join(', ')}`, `Low space: ${system.low_space.join(', ')}`) : '',
          ]
            .filter(Boolean)
            .join('\n')}
        />
        <div className="flex items-center gap-1">
          <Notifications />
          <Profile />
        </div>
      </div>
    </header>
  )
}
