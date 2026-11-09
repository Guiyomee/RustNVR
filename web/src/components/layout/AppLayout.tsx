import { CircleArrowUp, CirclePause, FlaskConical, RefreshCw, ShieldAlert, X } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import { describeError } from '../../api/client'
import { setDetectionPaused } from '../../api/endpoints'
import { useReadOnly } from '../../auth/AuthGate'
import { useData, useLive } from '../../data/DataContext'
import { useStoredFlag } from '../../hooks/useUi'
import { Link } from '../../lib/router'
import { Sidebar } from './Sidebar'
import { Topbar } from './Topbar'
import { t } from '../../lib/i18n'

function DemoBanner() {
  return (
    <div role="status" className="flex flex-wrap items-center justify-center gap-x-3 gap-y-1 bg-warn px-4 py-1.5 text-center text-xs font-semibold text-bg-2">
      <FlaskConical className="size-4" aria-hidden />
      {t("DÉMO — caméras, images et événements fictifs : rien n'est enregistré ni envoyé.", 'DEMO — fictional cameras, pictures and events: nothing is saved or sent.')}
      <a className="underline underline-offset-2" href="https://github.com/Guiyomee/RustNVR" target="_blank" rel="noreferrer">
        {t('Installer RustNVR', 'Install RustNVR')}
      </a>
    </div>
  )
}

function NoAuthBanner() {
  const auth = useLive((s) => s.system?.auth_enabled)
  if (auth !== false) return null
  return (
    <div role="alert" className="flex items-center justify-center gap-2 border-b border-warn/40 bg-warn/10 px-4 py-1.5 text-center text-xs text-warn">
      <ShieldAlert className="size-4 shrink-0" aria-hidden />
      {t("Interface accessible sans authentification. Configurez web.username et web.password_env avant de l'exposer au réseau.", "Interface reachable without authentication. Set web.username and web.password_env before exposing it to the network.")}
    </div>
  )
}

/** Motion detection paused on every camera (Home Assistant « Détection » switch). */
function PausedBanner() {
  const paused = useLive((s) => s.cameras.some((c) => c.motion.paused))
  const readOnly = useReadOnly()
  const { source } = useData()
  const [busy, setBusy] = useState(false)
  if (!paused) return null
  const resume = async () => {
    setBusy(true)
    try {
      await setDetectionPaused(false)
    } catch (e) {
      source.store.notify('error', describeError(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div role="status" className="flex flex-wrap items-center justify-center gap-x-3 gap-y-1 border-b border-warn/40 bg-warn/10 px-4 py-1.5 text-center text-xs text-warn">
      <CirclePause className="size-4 shrink-0" aria-hidden />
      {t('Détection de mouvement en pause sur toutes les caméras : pas d’événements, et rien n’est enregistré par les caméras « sur mouvement ».', 'Motion detection paused on every camera: no events, and cameras recording « on motion » record nothing.')}
      {readOnly ? (
        <span>{t('Un administrateur peut la reprendre.', 'An administrator can resume it.')}</span>
      ) : (
        <button type="button" className="rounded border border-warn/60 px-2 py-0.5 font-medium hover:bg-warn/20" onClick={() => void resume()}>
          {busy ? t('Reprise…', 'Resuming…') : t('Reprendre', 'Resume')}
        </button>
      )}
    </div>
  )
}

/** An update was installed while the server runs: the interface may already be the new one. */
function RestartBanner() {
  const needed = useLive((s) => s.system?.restart_needed)
  const restored = useLive((s) => s.system?.config_restored)
  if (restored && !needed) {
    return (
      <div role="alert" className="flex flex-wrap items-center justify-center gap-x-2 gap-y-1 border-b border-accent/40 bg-accent/10 px-4 py-1.5 text-center text-xs text-accent">
        <RefreshCw className="size-4 shrink-0" aria-hidden />
        {t('Configuration restaurée : redémarrez RustNVR pour l’utiliser (', 'Configuration restored: restart RustNVR to use it (')}
        <code className="font-mono">docker restart rustnvr</code>{t(', ou ', ', or ')}<code className="font-mono">systemctl restart rustnvr</code>
        {t('). D’ici là, RustNVR garde la configuration précédente.', '). Until then, RustNVR keeps the previous configuration.')}
      </div>
    )
  }
  if (!needed) return null
  return (
    <div role="alert" className="flex flex-wrap items-center justify-center gap-x-2 gap-y-1 border-b border-accent/40 bg-accent/10 px-4 py-1.5 text-center text-xs text-accent">
      <RefreshCw className="size-4 shrink-0" aria-hidden />
      {t("Une nouvelle version de RustNVR est installée : redémarrez le serveur pour l'utiliser (Ctrl+C puis", "A new version of RustNVR is installed: restart the server to use it (Ctrl+C then")}
      <code className="font-mono">rustnvr run -c config.toml</code>{t(', ou ', ', or ')}<code className="font-mono">systemctl restart rustnvr</code>{t("). D'ici là, certaines fonctions de l'interface peuvent échouer.", "). Until then, some functions of the interface may fail.")}
    </div>
  )
}

const DISMISSED_UPDATE_KEY = 'rustnvr.update.dismissed'

/** A newer version is published: once per version, until closed. */
function UpdateBanner() {
  const version = useLive((s) => s.system?.update_available ?? null)
  const [dismissed, setDismissed] = useState(() => window.localStorage.getItem(DISMISSED_UPDATE_KEY))
  if (!version || dismissed === version) return null
  return (
    <div role="status" className="flex flex-wrap items-center justify-center gap-x-3 gap-y-1 border-b border-warn/40 bg-warn/10 px-4 py-1.5 text-center text-xs text-warn">
      <CircleArrowUp className="size-4 shrink-0" aria-hidden />
      RustNVR {version}{t(' est disponible.', ' is available.')}
      <Link to="/about" className="font-medium underline">
        {t("Voir les nouveautés et comment mettre à jour", "See what's new and how to update")}
      </Link>
      <button
        type="button"
        aria-label={t('Masquer jusqu’à la prochaine version', 'Hide until the next version')}
        title={t('Masquer jusqu’à la prochaine version', 'Hide until the next version')}
        className="rounded p-0.5 hover:bg-warn/20"
        onClick={() => {
          window.localStorage.setItem(DISMISSED_UPDATE_KEY, version)
          setDismissed(version)
        }}
      >
        <X className="size-3.5" aria-hidden />
      </button>
    </div>
  )
}

export function AppLayout({ children }: { children: ReactNode }) {
  const { mode } = useData()
  const [collapsed, setCollapsed] = useStoredFlag('rustnvr.sidebar.collapsed', false)
  const [mobileOpen, setMobileOpen] = useState(false)
  return (
    <div className="flex min-h-screen">
      <Sidebar collapsed={collapsed} onToggleCollapsed={() => setCollapsed(!collapsed)} mobileOpen={mobileOpen} onCloseMobile={() => setMobileOpen(false)} />
      <div className="flex min-w-0 flex-1 flex-col">
        {mode === 'demo' ? (
          <DemoBanner />
        ) : (
          <>
            <RestartBanner />
            <UpdateBanner />
            <PausedBanner />
            <NoAuthBanner />
          </>
        )}
        <Topbar onOpenMenu={() => setMobileOpen(true)} />
        <main className="mx-auto w-full max-w-[1920px] flex-1 p-4 lg:p-6">{children}</main>
      </div>
    </div>
  )
}
