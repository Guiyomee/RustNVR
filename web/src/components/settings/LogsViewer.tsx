import { Pause, Play, Trash2 } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { describeError } from '../../api/client'
import { getLogs } from '../../api/endpoints'
import { useAuth } from '../../auth/AuthGate'
import { useIsDemo, useLive } from '../../data/DataContext'
import type { LogEntry, LogLevel } from '../../types/api'
import { Button } from '../ui/Button'
import { Segmented } from '../ui/Tabs'
import { locale, t } from '../../lib/i18n'

/** Messages kept in the page. */
const KEPT = 2000
const REFRESH_MS = 2000

const LEVELS: { value: LogLevel; label: string }[] = [
  { value: 'error', label: t('Erreurs', 'Errors') },
  { value: 'warn', label: t('Avertissements', 'Warnings') },
  { value: 'info', label: t('Infos', 'Info') },
  { value: 'debug', label: t('Détails', 'Details') },
]

const LEVEL_STYLE: Record<LogLevel, string> = {
  error: 'text-err',
  warn: 'text-warn',
  info: 'text-fg',
  debug: 'text-muted',
  trace: 'text-muted',
}

const LEVEL_TAG: Record<LogLevel, string> = { error: t('ERREUR', 'ERROR'), warn: t('ALERTE', 'WARNING'), info: 'INFO', debug: t('DÉTAIL', 'DETAIL'), trace: 'TRACE' }

function time(ms: number): string {
  return new Date(ms).toLocaleTimeString(locale(), { hour12: false }) + `.${String(ms % 1000).padStart(3, '0')}`
}

/** Paramètres → Journaux: latest messages of RustNVR, live. */
export function LogsViewer() {
  const demo = useIsDemo()
  const { user } = useAuth()
  const cameras = useLive((s) => s.cameras)
  const [level, setLevel] = useState<LogLevel>('info')
  const [camera, setCamera] = useState('')
  const [search, setSearch] = useState('')
  const [paused, setPaused] = useState(false)
  const [entries, setEntries] = useState<LogEntry[]>([])
  const [error, setError] = useState<string | null>(null)
  const since = useRef(0)
  const list = useRef<HTMLDivElement>(null)
  const stick = useRef(true)

  // Restart from scratch when the filters change.
  useEffect(() => {
    since.current = 0
    setEntries([])
  }, [level, camera])

  useEffect(() => {
    if (demo || paused) return
    let alive = true
    const load = async () => {
      try {
        const r = await getLogs({ since: since.current, level, camera: camera || undefined, limit: since.current === 0 ? 500 : 1000 })
        if (!alive) return
        since.current = r.last_seq
        setError(null)
        if (r.entries.length > 0) setEntries((e) => [...e, ...r.entries].slice(-KEPT))
      } catch (e) {
        if (alive) setError(describeError(e))
      }
    }
    void load()
    const id = window.setInterval(() => void load(), REFRESH_MS)
    return () => {
      alive = false
      window.clearInterval(id)
    }
  }, [demo, paused, level, camera])

  // Follow the newest messages unless the reader scrolled up.
  useEffect(() => {
    const el = list.current
    if (el && stick.current) el.scrollTop = el.scrollHeight
  }, [entries])

  const shown = useMemo(() => {
    const q = search.trim().toLowerCase()
    return q ? entries.filter((e) => e.message.toLowerCase().includes(q) || e.target.toLowerCase().includes(q)) : entries
  }, [entries, search])

  if (demo) return <p className="px-5 py-4 text-sm text-muted">{t('Indisponible en mode démo.', 'Not available in demo mode.')}</p>
  if (user && user.role !== 'admin') return <p className="px-5 py-4 text-sm text-muted">{t('Le journal est réservé aux administrateurs.', 'The log is for administrators only.')}</p>
  const names = new Map(cameras.map((c) => [c.id, c.name]))

  return (
    <div className="space-y-3 px-5 py-4">
      <div className="flex flex-wrap items-center gap-3 text-sm">
        <Segmented label={t('Niveau minimal', 'Minimum level')} options={LEVELS} value={level} onChange={setLevel} />
        <select className="h-8 rounded-md border border-line bg-bg-2 px-2 text-sm" value={camera} onChange={(e) => setCamera(e.target.value)} aria-label={t('Caméra', 'Camera')}>
          <option value="">{t('Toutes les caméras', 'All cameras')}</option>
          {cameras.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
        <input className="h-8 w-56 rounded-md border border-line bg-bg-2 px-2 text-sm" placeholder={t('Rechercher…', 'Search…')} value={search} onChange={(e) => setSearch(e.target.value)} />
        <Button size="sm" icon={paused ? <Play className="size-3.5" /> : <Pause className="size-3.5" />} onClick={() => setPaused(!paused)}>
          {paused ? t('Reprendre', 'Resume') : t('Pause', 'Pause')}
        </Button>
        <Button size="sm" icon={<Trash2 className="size-3.5" />} onClick={() => setEntries([])}>
          {t("Vider l'affichage", "Clear the view")}
        </Button>
        <span className="ml-auto text-xs text-muted tabular">{shown.length}{t(' message(s)', ' message(s)')}{paused ? t(' · en pause', ' · paused') : t(' · en direct', ' · live')}</span>
      </div>
      {error && <p className="text-sm text-err">{error}</p>}
      <div
        ref={list}
        onScroll={(e) => {
          const el = e.currentTarget
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40
        }}
        className="h-[60vh] overflow-auto rounded-md border border-line bg-bg-2 p-2 font-mono text-[12px] leading-5"
      >
        {shown.length === 0 ? (
          <p className="p-2 text-muted">{t('Aucun message pour ces filtres.', 'No message for these filters.')}</p>
        ) : (
          shown.map((e) => (
            <div key={e.seq} className={`whitespace-pre-wrap break-words ${LEVEL_STYLE[e.level]}`}>
              <span className="text-muted">{time(e.time_ms)}</span> <span className="font-semibold">{LEVEL_TAG[e.level]}</span>
              {e.camera && <span className="text-accent"> [{names.get(e.camera) ?? e.camera}]</span>} <span>{e.message}</span>
              <span className="text-muted"> · {e.target}</span>
            </div>
          ))
        )}
      </div>
      <p className="text-xs text-muted">
        {t('Les ', 'The ')}{KEPT}{t(" derniers messages depuis le démarrage, comme dans le journal de la console (jamais de mot de passe ni d'adresse complète de caméra). Les détails n'apparaissent que si RustNVR est lancé avec RUST_LOG=debug.", " latest messages since startup, as in the console log (never a password nor a full camera address). Details only appear when RustNVR runs with RUST_LOG=debug.")}
      </p>
    </div>
  )
}
