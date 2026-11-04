import { Component, lazy, Suspense, type ComponentType, type ReactNode } from 'react'
import { AppLayout } from './components/layout/AppLayout'
import { Button } from './components/ui/Button'
import { Card } from './components/ui/Card'
import { EmptyState, LoadingState } from './components/ui/States'
import { AuthGate } from './auth/AuthGate'
import { DataProvider } from './data/DataContext'
import { Link, matchPath, useLocation } from './lib/router'
import { OverviewPage } from './pages/OverviewPage'
import { t } from './lib/i18n'

const RELOADED_KEY = 'rustnvr.reloaded-for-update'

/**
 * A page loaded on demand. After an update of the interface the files of
 * an already open tab no longer exist: reload once to get the new ones
 * instead of showing a blank page.
 */
function lazyPage<P>(load: () => Promise<ComponentType<P>>) {
  return lazy(() =>
    load().then(
      (component) => {
        window.sessionStorage.removeItem(RELOADED_KEY)
        return { default: component }
      },
      (error: unknown) => {
        if (!window.sessionStorage.getItem(RELOADED_KEY)) {
          window.sessionStorage.setItem(RELOADED_KEY, '1')
          window.location.reload()
          return new Promise<never>(() => undefined)
        }
        throw error
      },
    ),
  )
}

/** Shows a way out instead of a blank page if a page fails to load or render. */
class PageBoundary extends Component<{ children: ReactNode; path: string }, { error: Error | null; path: string }> {
  state = { error: null as Error | null, path: this.props.path }
  static getDerivedStateFromError(error: Error) {
    return { error }
  }
  static getDerivedStateFromProps(props: { path: string }, state: { error: Error | null; path: string }) {
    // Navigating elsewhere clears the error.
    return props.path === state.path ? null : { error: null, path: props.path }
  }
  render() {
    if (!this.state.error) return this.props.children
    return (
      <Card>
        <EmptyState title={t("Cette page n'a pas pu s'afficher", "This page could not be shown")}>
          <span className="block">{this.state.error.message}</span>
          <span className="mt-3 block">
            <Button variant="primary" onClick={() => window.location.reload()}>
              {t('Recharger', 'Reload')}
            </Button>
          </span>
        </EmptyState>
      </Card>
    )
  }
}

// The dashboard is in the main bundle; other pages load on demand.
const CamerasPage = lazyPage(() => import('./pages/CamerasPage').then((m) => m.CamerasPage))
const CameraDetailPage = lazyPage(() => import('./pages/CameraDetailPage').then((m) => m.CameraDetailPage))
const EventsPage = lazyPage(() => import('./pages/EventsPage').then((m) => m.EventsPage))
const PlaybackPage = lazyPage(() => import('./pages/PlaybackPage').then((m) => m.PlaybackPage))
const StoragePage = lazyPage(() => import('./pages/StoragePage').then((m) => m.StoragePage))
const SettingsPage = lazyPage(() => import('./pages/SettingsPage').then((m) => m.SettingsPage))
const AboutPage = lazyPage(() => import('./pages/AboutPage').then((m) => m.AboutPage))
const WallPage = lazyPage(() => import('./pages/WallPage').then((m) => m.WallPage))

/** The wall of screens is shown alone (no menus); every other page in the layout. */
function Shell({ mode }: { mode: string }) {
  const { path } = useLocation()
  if (path === '/mur')
    return (
      <PageBoundary path={path} key={mode}>
        <Suspense fallback={<LoadingState />}>
          <WallPage />
        </Suspense>
      </PageBoundary>
    )
  return (
    <AppLayout key={mode}>
      <Routes />
    </AppLayout>
  )
}

function Routes() {
  const { path } = useLocation()
  let page: ReactNode
  const detail = matchPath('/cameras/:id', path)
  if (path === '/') page = <OverviewPage />
  else if (path === '/cameras') page = <CamerasPage />
  else if (detail) page = <CameraDetailPage key={detail.id} id={detail.id ?? ''} />
  else if (path === '/events') page = <EventsPage />
  else if (path === '/playback') page = <PlaybackPage />
  else if (path === '/storage') page = <StoragePage />
  else if (path === '/settings') page = <SettingsPage />
  else if (path === '/about') page = <AboutPage />
  else
    page = (
      <Card>
        <EmptyState title={t('Page introuvable', 'Page not found')}>
          <Link to="/" className="text-accent hover:underline">
            {t("Retour à la vue d'ensemble", "Back to the overview")}
          </Link>
        </EmptyState>
      </Card>
    )
  return (
    <PageBoundary path={path}>
      <Suspense fallback={<LoadingState />}>{page}</Suspense>
    </PageBoundary>
  )
}

export function App() {
  return (
    <AuthGate>
      <DataProvider>
      {/* Remount everything when switching live/demo so that no state crosses over. */}
      {(mode) => <Shell mode={mode} />}
      </DataProvider>
    </AuthGate>
  )
}
