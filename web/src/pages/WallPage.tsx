import { LogOut, Maximize, Settings2, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { CameraPlayer } from '../components/cameras/CameraPlayer'
import { Segmented } from '../components/ui/Tabs'
import { useLive } from '../data/DataContext'
import { useNow } from '../hooks/useUi'
import { formatTime } from '../lib/format'
import { Link, navigate, useLocation } from '../lib/router'
import type { Camera } from '../types/api'
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

/** Order chosen on the overview (same browser), if any. */
function overviewOrder(): string[] {
  try {
    const v: unknown = JSON.parse(window.localStorage.getItem('rustnvr.overview.order') ?? '[]')
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []
  } catch {
    return []
  }
}

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

  // Settings in the address, so that a tablet can keep them as a bookmark.
  const chosen = query.get('cameras')?.split(',').filter(Boolean) ?? null
  const colsParam = query.get('cols')
  const columns: Columns = colsParam === '1' || colsParam === '2' || colsParam === '3' || colsParam === '4' ? (Number(colsParam) as Columns) : 'auto'
  const focusOn = query.get('focus') !== '0'
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

  const cameras = useMemo(() => {
    const enabled = all.filter((c) => c.enabled)
    if (chosen) return chosen.map((id) => enabled.find((c) => c.id === id)).filter((c): c is Camera => Boolean(c))
    const rank = new Map(overviewOrder().map((id, i) => [id, i]))
    return [...enabled].sort((a, b) => (rank.get(a.id) ?? 1e9) - (rank.get(b.id) ?? 1e9))
  }, [all, chosen?.join(',')])

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
        <div className="flex h-full items-center justify-center text-sm text-muted">{t('Aucune caméra à afficher.', 'No camera to show.')}</div>
      ) : (
        <div className="grid h-full w-full gap-0.5" style={{ gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`, gridTemplateRows: `repeat(${rows}, minmax(0, 1fr))` }}>
          {cameras.map((c) => {
            const big = enlarge && c.id === focused
            return (
              // Same order in the page whatever the focus (CSS `order` puts the
              // enlarged one first): moving a playing <video> stops its stream.
              <div key={c.id} style={{ order: big ? -1 : 0 }} className={`relative min-h-0 min-w-0 overflow-hidden bg-bg-2 ${big ? 'col-span-2 row-span-2 ring-2 ring-warn ring-inset' : ''}`}>
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
                        // All of them, in the overview order: no need to keep a list.
                        setParams({ cameras: next.length === enabledIds.length && next.every((x) => enabledIds.includes(x)) ? null : next })
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
            <Segmented label={t('Caméras par ligne', 'Cameras per row')} options={COLUMNS} value={columns} onChange={(v) => setParams({ cols: v })} />
          </div>
          <label className="flex items-start gap-2">
            <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={focusOn} onChange={(e) => setParams({ focus: e.target.checked })} />
            <span>
              {t('Agrandir la caméra en mouvement', 'Enlarge the camera with motion')}
              <span className="block text-xs text-muted">{t('Pendant le mouvement et 10 s après.', 'During motion and 10 s after.')}</span>
            </span>
          </label>
          <p className="text-xs text-muted">{t('Ces réglages sont dans l’adresse de la page : ajoutez-la aux favoris (ou à l’écran d’accueil de la tablette) pour la retrouver telle quelle. L’écran reste allumé et la page se reconnecte seule.', 'These settings are in the page address: bookmark it (or add it to the tablet home screen) to get it back as it is. The screen stays on and the page reconnects by itself.')}</p>
        </div>
      )}
    </div>
  )
}
