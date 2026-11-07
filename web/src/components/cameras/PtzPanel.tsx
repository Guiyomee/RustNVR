import {
  ArrowDown,
  ArrowDownLeft,
  ArrowDownRight,
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  ArrowUpLeft,
  ArrowUpRight,
  BookmarkPlus,
  MapPin,
  Square,
  Trash2,
  ZoomIn,
  ZoomOut,
} from 'lucide-react'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { useReadOnly } from '../../auth/AuthGate'
import { describeError } from '../../api/client'
import { getPtz, ptzDeletePreset, ptzGoto, ptzMove, ptzSavePreset, ptzStop } from '../../api/endpoints'
import { useData, useIsDemo } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { t } from '../../lib/i18n'
import type { Camera, PtzMove, PtzPreset } from '../../types/api'
import { Button, IconButton } from '../ui/Button'
import { Card, CardHeader } from '../ui/Card'
import { Segmented } from '../ui/Tabs'

const SPEEDS = { slow: 0.2, normal: 0.5, fast: 1 } as const
type Speed = keyof typeof SPEEDS

/** Positions shown in demo mode (nothing is sent). */
const DEMO_PRESETS: PtzPreset[] = [
  { token: '1', name: t('Portail', 'Gate') },
  { token: '2', name: t('Allée', 'Driveway') },
  { token: '3', name: t('Terrasse', 'Terrace') },
]

/** Pan, tilt, zoom and presets of a motorised camera (ONVIF PTZ). */
export function PtzPanel({ camera }: { camera: Camera }) {
  const demo = useIsDemo()
  const readOnly = useReadOnly()
  const { source } = useData()
  const loaded = useAsync(() => (demo ? Promise.resolve({ presets: DEMO_PRESETS }) : getPtz(camera.id)), [camera.id, demo])
  const [speed, setSpeed] = useState<Speed>('normal')
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const moving = useRef(false)

  const blocked = demo ? t('Indisponible en mode démo : la caméra n’est pas pilotée.', 'Not available in demo mode: the camera is not driven.') : readOnly
  const fail = (err: unknown) => source.store.notify('error', `${camera.name} : ${describeError(err)}`, camera.id)

  const stop = () => {
    if (!moving.current) return
    moving.current = false
    ptzStop(camera.id).catch(fail)
  }
  const start = (dir: PtzMove) => {
    if (blocked) return
    const v = SPEEDS[speed]
    moving.current = true
    ptzMove(camera.id, { pan: (dir.pan ?? 0) * v, tilt: (dir.tilt ?? 0) * v, zoom: (dir.zoom ?? 0) * v }).catch((err) => {
      moving.current = false
      fail(err)
    })
  }
  // Never leave the camera moving: page left, tab hidden.
  useEffect(() => {
    const hidden = () => {
      if (document.hidden) stop()
    }
    document.addEventListener('visibilitychange', hidden)
    return () => {
      document.removeEventListener('visibilitychange', hidden)
      stop()
    }
  }, [camera.id])

  const act = async (run: () => Promise<unknown>, reload = false) => {
    setBusy(true)
    try {
      await run()
      if (reload) loaded.reload()
    } catch (err) {
      fail(err)
    } finally {
      setBusy(false)
    }
  }

  /** A button moving the camera while held (mouse, touch, or keyboard). */
  const hold = (label: string, icon: ReactNode, dir: PtzMove) => (
    <IconButton
      label={label}
      icon={icon}
      disabledReason={blocked}
      className="size-10 border border-line bg-bg-2"
      onPointerDown={(e) => {
        e.currentTarget.setPointerCapture(e.pointerId)
        start(dir)
      }}
      onPointerUp={stop}
      onPointerCancel={stop}
      onLostPointerCapture={stop}
      onKeyDown={(e) => {
        if ((e.key === 'Enter' || e.key === ' ') && !e.repeat) {
          e.preventDefault()
          start(dir)
        }
      }}
      onKeyUp={(e) => {
        if (e.key === 'Enter' || e.key === ' ') stop()
      }}
      onBlur={stop}
    />
  )

  const presets = loaded.data?.presets ?? []
  return (
    <Card>
      <CardHeader
        title={t('Pilotage PTZ', 'PTZ control')}
        subtitle={t('Maintenir un bouton pour déplacer', 'Hold a button to move')}
      />
      {loaded.error ? (
        <div className="space-y-2 px-4 py-3 text-sm">
          <p className="text-err">{describeError(loaded.error)}</p>
          <Button size="sm" onClick={loaded.reload}>
            {t('Réessayer', 'Retry')}
          </Button>
        </div>
      ) : (
        <div className="space-y-4 px-4 py-3">
          <div className="flex items-center justify-center gap-6">
            <div className="grid grid-cols-3 gap-1.5" role="group" aria-label={t('Déplacement', 'Move')}>
              {hold(t('Haut gauche', 'Up left'), <ArrowUpLeft className="size-4" />, { pan: -1, tilt: 1 })}
              {hold(t('Haut', 'Up'), <ArrowUp className="size-4" />, { tilt: 1 })}
              {hold(t('Haut droite', 'Up right'), <ArrowUpRight className="size-4" />, { pan: 1, tilt: 1 })}
              {hold(t('Gauche', 'Left'), <ArrowLeft className="size-4" />, { pan: -1 })}
              <IconButton
                label={t('Arrêter', 'Stop')}
                icon={<Square className="size-3.5" />}
                disabledReason={blocked}
                className="size-10 border border-line bg-bg-2"
                onClick={() => {
                  moving.current = true
                  stop()
                }}
              />
              {hold(t('Droite', 'Right'), <ArrowRight className="size-4" />, { pan: 1 })}
              {hold(t('Bas gauche', 'Down left'), <ArrowDownLeft className="size-4" />, { pan: -1, tilt: -1 })}
              {hold(t('Bas', 'Down'), <ArrowDown className="size-4" />, { tilt: -1 })}
              {hold(t('Bas droite', 'Down right'), <ArrowDownRight className="size-4" />, { pan: 1, tilt: -1 })}
            </div>
            <div className="flex flex-col gap-1.5" role="group" aria-label={t('Zoom', 'Zoom')}>
              {hold(t('Zoom avant', 'Zoom in'), <ZoomIn className="size-4" />, { zoom: 1 })}
              {hold(t('Zoom arrière', 'Zoom out'), <ZoomOut className="size-4" />, { zoom: -1 })}
            </div>
          </div>
          <div className="flex items-center justify-between gap-2 text-xs text-muted">
            {t('Vitesse', 'Speed')}
            <Segmented
              label={t('Vitesse', 'Speed')}
              options={[
                { value: 'slow', label: t('Lente', 'Slow') },
                { value: 'normal', label: t('Normale', 'Normal') },
                { value: 'fast', label: t('Rapide', 'Fast') },
              ]}
              value={speed}
              onChange={setSpeed}
            />
          </div>

          <div>
            <h3 className="mb-1.5 text-xs font-medium text-muted">{t('Positions enregistrées', 'Stored positions')}</h3>
            {!loaded.data ? (
              <p className="text-xs text-muted">{t('Lecture des positions…', 'Reading the positions…')}</p>
            ) : presets.length === 0 ? (
              <p className="text-xs text-muted">{t('Aucune position enregistrée sur la caméra.', 'No position stored on the camera.')}</p>
            ) : (
              <ul className="max-h-56 space-y-1 overflow-y-auto">
                {presets.map((p) => (
                  <li key={p.token} className="flex items-center gap-2">
                    <Button
                      size="sm"
                      className="min-w-0 flex-1 justify-start"
                      icon={<MapPin className="size-3.5" />}
                      disabledReason={blocked ?? (busy ? t('Commande en cours…', 'Command running…') : null)}
                      onClick={() => void act(() => ptzGoto(camera.id, p.token))}
                    >
                      <span className="truncate">{p.name}</span>
                    </Button>
                    <IconButton
                      label={t(`Supprimer « ${p.name} »`, `Delete "${p.name}"`)}
                      icon={<Trash2 className="size-3.5" />}
                      disabledReason={blocked ?? (busy ? t('Commande en cours…', 'Command running…') : null)}
                      onClick={() => {
                        if (window.confirm(t(`Supprimer la position « ${p.name} » de la caméra ?`, `Delete the position "${p.name}" from the camera?`))) {
                          void act(() => ptzDeletePreset(camera.id, p.token), true)
                        }
                      }}
                    />
                  </li>
                ))}
              </ul>
            )}
          </div>
          <form
            className="flex gap-2"
            onSubmit={(e) => {
              e.preventDefault()
              const n = name.trim()
              if (!n) return
              void act(async () => {
                await ptzSavePreset(camera.id, n)
                setName('')
              }, true)
            }}
          >
            <input
              className="h-8 min-w-0 flex-1 rounded-md border border-line bg-bg-2 px-2 text-sm outline-none focus:border-accent"
              placeholder={t('Nouvelle position', 'New position')}
              aria-label={t('Nom de la position actuelle', 'Name of the current position')}
              maxLength={64}
              value={name}
              disabled={Boolean(blocked)}
              onChange={(e) => setName(e.target.value)}
            />
            <Button
              type="submit"
              size="sm"
              icon={<BookmarkPlus className="size-3.5" />}
              disabledReason={blocked ?? (!name.trim() ? t('Donnez un nom à la position.', 'Name the position.') : busy ? t('Commande en cours…', 'Command running…') : null)}
            >
              {t('Enregistrer', 'Save')}
            </Button>
          </form>
        </div>
      )}
    </Card>
  )
}
