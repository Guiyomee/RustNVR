import { LayoutGrid, LogOut, Maximize, Settings2, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { CameraPlayer } from '../components/cameras/CameraPlayer'
import { Segmented } from '../components/ui/Tabs'
import { Button } from '../components/ui/Button'
import { getWall, setWall } from '../api/endpoints'
import { describeError } from '../api/client'
import { useIsViewer } from '../auth/AuthGate'
import { useIsDemo, useLive } from '../data/DataContext'
import { useAsync } from '../hooks/useAsync'
import { useNow } from '../hooks/useUi'
import { formatTime } from '../lib/format'
import { Link, navigate, useLocation } from '../lib/router'
import type { Camera, WallSettings } from '../types/api'
import { t } from '../lib/i18n'

type Columns = 'auto' | 1 | 2 | 3 | 4
const COLUMNS: { value: Columns; label: string }[] = [
  { value: 'auto', label: 'Auto' },
  { value: 1, label: '1' },
  { value: 2, label: '2' },
  { value: 3, label: '3' },
  { value: 4, label: '4' },
]
/** A camera stays enlarged this long after its motion ended. */
const FOCUS_HOLD_MS = 10_000
/** Without connection to the server this long, the page reloads itself. */
const RELOAD_AFTER_MS = 120_000
/** Controls disappear after this long without moving the mouse or touching. */
const CONTROLS_MS = 4_000

/** Every camera, automatic layout, enlarged motion: before anything is chosen. */
const DEFAULT_WALL: WallSettings = { cameras: [], columns: null, focus: true }

const toColumns = (n: number | null | undefined): Columns => (n === 1 || n === 2 || n === 3 || n === 4 ? n : 'auto')

/** Keeps the screen on while the wall is shown (browsers that allow it). */
function useWakeLock() {
  useEffect(() => {
    type Sentinel = { release: () => Promise<void> }
    const nav = navigator as Navigator & { wakeLock?: { request: (t: 'screen') => Promise<Sentinel> } }
    let lock: Sentinel | null = null
    const take = () => {
      if (document.visibilityState === 'visible' && nav.wakeLock) void nav.wakeLock.request('screen').then((l) => (lock = l), () => undefined)
    }
    take()
    document.addEventListener('visibilitychange', take)
    return () => {
      document.removeEventListener('visibilitychange', take)
      void lock?.release().catch(() => undefined)
    }
  }, [])
}

/** Mur d'écrans : the chosen cameras live, full screen, without menus. */
export function WallPage() {
  const { query } = useLocation()
  const all = useLive((s) => s.cameras)
  const connection = useLive((s) => s.connection)
  const now = useNow(1_000)
  const root = useRef<HTMLDivElement>(null)
  useWakeLock()

  // The general settings (server, every device), unless the address of this
  // screen has its own (a tablet bookmark). An administrator changes the
  // general ones; a viewer, or a screen with its own, changes the address.
  const demo = useIsDemo()
  const viewer = useIsViewer()
  const loaded = useAsync((signal) => (demo ? Promise.resolve(DEFAULT_WALL) : getWall(signal)), [demo])
  const [saved, setSaved] = useState<WallSettings | null>(null)
  const [saveError, setSaveError] = useState<string | null>(null)
  const general = saved ?? loaded.data ?? DEFAULT_WALL
  const own = query.has('cameras') || query.has('cols') || query.has('focus')
  // No camera shown before the general settings are known: players would
  // start, then stop when they arrive.
  const waiting = !own && !demo && saved === null && loaded.data === null && !loaded.error
  const canSave = !demo && !viewer
  const chosen = own ? (query.get('cameras')?.split(',').filter(Boolean) ?? null) : general.cameras.length > 0 ? general.cameras : null
  const columns: Columns = own ? toColumns(Number(query.get('cols'))) : toColumns(general.columns)
  const focusOn = own ? query.get('focus') !== '0' : general.focus
  const save = (wall: WallSettings) => {
    setSaveError(null)
    setWall(wall).then(
      (w) => {
        setSaved(w)
        // This screen follows the general settings from now on.
        if (own) navigate('/mur', { replace: true })
      },
      (e: unknown) => setSaveError(describeError(e)),
    )
  }
  const apply = (next: { cameras?: string[] | null; cols?: Columns; focus?: boolean }) => {
    if (own || !canSave) return setParams(next)
    const cols = next.cols ?? columns
    save({ cameras: (next.cameras === undefined ? chosen : next.cameras) ?? [], columns: cols === 'auto' ? null : cols, focus: next.focus ?? focusOn })
  }
  const setParams = (next: { cameras?: string[] | null; cols?: Columns; focus?: boolean }) => {
    const p = new URLSearchParams()
    const cams = next.cameras === undefined ? chosen : next.cameras
    if (cams && cams.length > 0) p.set('cameras', cams.join(','))
    const cols = next.cols ?? columns
    if (cols !== 'auto') p.set('cols', String(cols))
    if (!(next.focus ?? focusOn)) p.set('focus', '0')
    const qs = p.toString()
    navigate(`/mur${qs ? `?${qs}` : ''}`, { replace: true })
  }

  // Without a choice, every camera in the order kept on the server.
  const wall = useMemo(() => {
    const enabled = all.filter((c) => c.enabled)
    if (waiting) return []
    if (chosen) return chosen.map((id) => enabled.find((c) => c.id === id)).filter((c): c is Camera => Boolean(c))
    return enabled
  }, [all, chosen?.join(','), waiting])
  // A camera clicked on is shown alone (the others stop); in the address, so
  // that the back button of the browser or the tablet returns to the wall.
  const soloId = query.get('solo')
  const solo = soloId ? wall.find((c) => c.id === soloId) : undefined
  const cameras = solo ? [solo] : wall
  // Opened from the wall: going back returns to it. Opened from a bookmark:
  // there is no wall to go back to, the address is changed instead.
  const openedHere = useRef(false)
  const openSolo = (id: string) => {
    const p = new URLSearchParams(window.location.search)
    p.set('solo', id)
    openedHere.current = true
    navigate(`/mur?${p.toString()}`)
  }
  const closeSolo = () => {
    if (openedHere.current) window.history.back()
    else setParams({})
    openedHere.current = false
  }
  useEffect(() => {
    if (!solo) return
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && closeSolo()
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [solo?.id])

  // Last time each camera saw motion: the most recent one is enlarged.
  const lastMotion = useRef(new Map<string, number>())
  for (const c of cameras) if (c.motion.active) lastMotion.current.set(c.id, now)
  const focused = focusOn
    ? cameras
        .filter((c) => now - (lastMotion.current.get(c.id) ?? -Infinity) < FOCUS_HOLD_MS)
        .sort((a, b) => (lastMotion.current.get(b.id) ?? 0) - (lastMotion.current.get(a.id) ?? 0))[0]?.id ?? null
    : null

  // Lost connection for a while (server restarted or updated): start again.
  const lostSince = useRef<number | null>(null)
  useEffect(() => {
    if (connection === 'open') lostSince.current = null
    else if (lostSince.current === null) lostSince.current = Date.now()
    else if (Date.now() - lostSince.current > RELOAD_AFTER_MS) window.location.reload()
  }, [connection, now])

  // Controls shown on mouse move or touch, hidden after a few seconds.
  const [controls, setControls] = useState(true)
  const [panel, setPanel] = useState(false)
  useEffect(() => {
    let timer = window.setTimeout(() => setControls(false), CONTROLS_MS)
    const show = () => {
      setControls(true)
      window.clearTimeout(timer)
      timer = window.setTimeout(() => setControls(false), CONTROLS_MS)
    }
    window.addEventListener('pointermove', show)
    window.addEventListener('pointerdown', show)
    return () => {
      window.clearTimeout(timer)
      window.removeEventListener('pointermove', show)
      window.removeEventListener('pointerdown', show)
    }
  }, [])

  const n = cameras.length
  const cols = columns === 'auto' ? Math.max(1, Math.ceil(Math.sqrt(n))) : Math.min(columns, Math.max(1, n))
  const enlarge = focused !== null && cols > 1 && n > 1
  const cells = n + (enlarge ? 3 : 0)
  const rows = Math.max(1, Math.ceil(cells / cols))
  const enabledIds = all.filter((c) => c.enabled).map((c) => c.id)
  const current = chosen ?? cameras.map((c) => c.id)

  return (
    <div ref={root} className={`fixed inset-0 bg-black text-fg ${controls ? '' : 'cursor-none'}`}>
      {n === 0 ? (
        <div className="flex h-full items-center justify-center text-sm text-muted">{waiting ? t('Chargement…', 'Loading…') : t('Aucune caméra à afficher.', 'No camera to show.')}</div>
      ) : (
        <div className="grid h-full w-full gap-0.5" style={{ gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`, gridTemplateRows: `repeat(${rows}, minmax(0, 1fr))` }}>
          {cameras.map((c) => {
            const big = enlarge && c.id === focused
            return (
              // Same order in the page whatever the focus (CSS `order` puts the
              // enlarged one first): moving a playing <video> stops its stream.
              <div
                key={c.id}
                style={{ order: big ? -1 : 0 }}
                role="button"
                tabIndex={0}
                aria-label={solo ? t('Revenir au mur', 'Back to the wall') : t(`Afficher ${c.name} seule`, `Show ${c.name} alone`)}
                title={solo ? t('Revenir au mur', 'Back to the wall') : t(`Afficher ${c.name} seule`, `Show ${c.name} alone`)}
                onClick={() => (solo ? closeSolo() : openSolo(c.id))}
                onKeyDown={(e) => e.key === 'Enter' && (solo ? closeSolo() : openSolo(c.id))}
                className={`relative min-h-0 min-w-0 cursor-pointer overflow-hidden bg-bg-2 ${big ? 'col-span-2 row-span-2 ring-2 ring-warn ring-inset' : ''}`}
              >
                <CameraPlayer camera={c} active large={n === 1} />
                <div className="pointer-events-none absolute top-1.5 left-1.5 flex gap-1">
                  <span className="rounded bg-black/60 px-1.5 py-0.5 text-xs font-semibold text-white">{c.name}</span>
                  {c.motion.active && <span className="rounded bg-warn px-1.5 py-0.5 text-xs font-semibold text-black">{t('Mouvement', 'Motion')}</span>}
                </div>
              </div>
            )
          })}
        </div>
      )}

      <div className="pointer-events-none absolute right-2 bottom-1.5 rounded bg-black/50 px-1.5 text-xs text-white/80 tabular">
        {connection === 'open' ? '' : t('Reconnexion… · ', 'Reconnecting… · ')}
        {formatTime(now, true)}
      </div>

      {(controls || panel) && (
        <div className="absolute top-2 right-2 flex gap-1.5">
          {solo && (
            <button type="button" title={t('Revenir au mur', 'Back to the wall')} aria-label={t('Revenir au mur', 'Back to the wall')} className="rounded-md bg-black/70 p-2 text-white hover:bg-black" onClick={closeSolo}>
              <LayoutGrid className="size-4" />
            </button>
          )}
          <button type="button" title={t('Plein écran', 'Full screen')} aria-label={t('Plein écran', 'Full screen')} className="rounded-md bg-black/70 p-2 text-white hover:bg-black" onClick={() => void (document.fullscreenElement ? document.exitFullscreen() : root.current?.requestFullscreen())?.catch(() => undefined)}>
            <Maximize className="size-4" />
          </button>
          <button type="button" title={t('Réglages du mur', 'Wall settings')} aria-label={t('Réglages du mur', 'Wall settings')} aria-pressed={panel} className="rounded-md bg-black/70 p-2 text-white hover:bg-black" onClick={() => setPanel(!panel)}>
            <Settings2 className="size-4" />
          </button>
          <Link to="/" className="rounded-md bg-black/70 p-2 text-white hover:bg-black" aria-label={t("Quitter le mur d'écrans", "Leave the video wall")}>
            <LogOut className="size-4" />
          </Link>
        </div>
      )}

      {panel && (
        <div className="absolute top-14 right-2 w-80 space-y-4 rounded-lg border border-line bg-bg/95 p-4 text-sm shadow-xl">
          <div className="flex items-center justify-between">
            <span className="font-semibold">{t('Mur d’écrans', 'Video wall')}</span>
            <button type="button" aria-label={t('Fermer', 'Close')} className="rounded p-1 hover:bg-bg-2" onClick={() => setPanel(false)}>
              <X className="size-4" />
            </button>
          </div>
          <div className="space-y-1.5">
            <span className="text-xs text-muted">{t('Caméras affichées', 'Cameras shown')}</span>
            <div className="flex flex-wrap gap-1.5">
              {all
                .filter((c) => c.enabled)
                .map((c) => {
                  const on = current.includes(c.id)
                  return (
                    <button
                      key={c.id}
                      type="button"
                      aria-pressed={on}
                      className={`rounded-full border px-2.5 py-0.5 text-xs ${on ? 'border-accent bg-accent/15' : 'border-line text-muted'}`}
                      onClick={() => {
                        const next = on ? current.filter((x) => x !== c.id) : [...current, c.id]
                        // All of them, in the cameras' order: no need to keep a list.
                        apply({ cameras: next.length === enabledIds.length && next.every((x) => enabledIds.includes(x)) ? null : next })
                      }}
                    >
                      {c.name}
                    </button>
                  )
                })}
            </div>
          </div>
          <div className="flex items-center justify-between gap-2">
            <span className="text-xs text-muted">{t('Par ligne', 'Per row')}</span>
            <Segmented label={t('Caméras par ligne', 'Cameras per row')} options={COLUMNS} value={columns} onChange={(v) => apply({ cols: v })} />
          </div>
          <label className="flex items-start gap-2">
            <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={focusOn} onChange={(e) => apply({ focus: e.target.checked })} />
            <span>
              {t('Agrandir la caméra en mouvement', 'Enlarge the camera with motion')}
              <span className="block text-xs text-muted">{t('Pendant le mouvement et 10 s après.', 'During motion and 10 s after.')}</span>
            </span>
          </label>
          <p className="text-xs text-muted">{t('Touchez une caméra pour l’afficher seule (avec son son, s’il en a), touchez-la encore ou faites « retour » pour revenir au mur.', 'Tap a camera to show it alone (with its sound, if any), tap it again or go back to return to the wall.')}</p>
          {own ? (
            <div className="space-y-2 rounded-md border border-line p-2.5 text-xs">
              <p className="text-muted">{t('Réglages propres à cet écran (dans l’adresse de la page, à garder en favori).', 'Settings of this screen only (in the page address, to keep as a bookmark).')}</p>
              <div className="flex flex-wrap gap-1.5">
                <Button size="sm" disabledReason={canSave ? null : demo ? t('Indisponible en mode démo.', 'Not available in demo mode.') : t('Seul un administrateur peut changer le réglage de tous les écrans.', 'Only an administrator can change the setting of every screen.')} onClick={() => save({ cameras: chosen ?? [], columns: columns === 'auto' ? null : columns, focus: focusOn })}>
                  {t('Utiliser pour tous les écrans', 'Use on every screen')}
                </Button>
                <Button size="sm" onClick={() => navigate('/mur', { replace: true })}>
                  {t('Reprendre le réglage général', 'Back to the general setting')}
                </Button>
              </div>
            </div>
          ) : (
            <p className="text-xs text-muted">
              {canSave
                ? t('Réglage général : il vaut pour tous les écrans, le panneau Home Assistant compris.', 'General setting: it applies to every screen, the Home Assistant panel included.')
                : t('Réglage général, choisi par un administrateur. Vos changements ne valent que pour cet écran (dans l’adresse de la page).', 'General setting, chosen by an administrator. Your changes apply to this screen only (in the page address).')}
            </p>
          )}
          {saveError && <p className="text-xs text-err">{saveError}</p>}
          <p className="text-xs text-muted">{t('L’écran reste allumé et la page se reconnecte seule.', 'The screen stays on and the page reconnects by itself.')}</p>
        </div>
      )}
    </div>
  )
}
