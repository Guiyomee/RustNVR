import { ArrowLeft, Camera as CameraIcon, CirclePlay, Download, Info, Maximize, Pencil, Radio, Settings2, Shapes, Video } from 'lucide-react'
import { useMemo, useRef, useState, type ReactNode } from 'react'
import { CameraForm } from '../components/cameras/CameraForm'
import { CameraPlayer } from '../components/cameras/CameraPlayer'
import { MotionPanel, MotionStatus } from '../components/cameras/MotionPanel'
import { PtzPanel } from '../components/cameras/PtzPanel'
import { RecordingBadge } from '../components/cameras/RecordingBadge'
import { Button } from '../components/ui/Button'
import { Badge, Card, CardHeader, InfoList } from '../components/ui/Card'
import { EmptyState, ErrorState, LoadingState } from '../components/ui/States'
import { CameraStatusBadge } from '../components/ui/StatusIndicator'
import { Segmented, Tabs } from '../components/ui/Tabs'
import { useReadOnly } from '../auth/AuthGate'
import { describeError } from '../api/client'
import { snapshotUrl } from '../api/endpoints'
import { useCapability, useData, useIsDemo, useLive } from '../data/DataContext'
import { useAsync } from '../hooks/useAsync'
import { useNow } from '../hooks/useUi'
import {
  formatBitrate,
  formatBytes,
  formatDateTime,
  destinationsLabel,
  formatDuration,
  formatNumber,
  formatResolution,
  formatTime,
  fromDateInput,
  startOfDay,
  toDateInput,
} from '../lib/format'
import { Link, navigate } from '../lib/router'
import { recordingToggleBlocker, setRecording } from '../lib/recording'
import type { LiveStream } from '../api/endpoints'
import type { Camera } from '../types/api'
import { t } from '../lib/i18n'

type TabKey = 'live' | 'recordings' | 'detection' | 'config' | 'tech'

const DAY = 86_400_000

function useDayRecordings(cameraId: string, day: number) {
  const { source } = useData()
  return useAsync((signal) => source.recordings(cameraId, day, day + DAY, signal), [source, cameraId, day])
}

function RecordingsTab({ camera }: { camera: Camera }) {
  const now = useNow(60_000)
  const [day, setDay] = useState(() => startOfDay(now))
  const list = useDayRecordings(camera.id, day)
  const total = list.data?.reduce((a, r) => a + r.duration_ms, 0) ?? 0
  return (
    <Card>
      <CardHeader
        title={t('Enregistrements', 'Recordings')}
        subtitle={list.data ? t(`${list.data.length} segments · ${formatDuration(total)}`, `${list.data.length} segments · ${formatDuration(total)}`) : undefined}
        actions={
          <input
            type="date"
            aria-label={t('Jour', 'Day')}
            className="h-8 rounded-md border border-line bg-bg-2 px-2 text-sm"
            value={toDateInput(day)}
            max={toDateInput(now)}
            onChange={(e) => {
              const d = fromDateInput(e.target.value)
              if (d !== null) setDay(d)
            }}
          />
        }
      />
      {list.error ? (
        <ErrorState error={list.error} onRetry={list.reload} />
      ) : !list.data ? (
        <LoadingState />
      ) : list.data.length === 0 ? (
        <EmptyState title={t('Aucun enregistrement ce jour-là', 'No recording on that day')} />
      ) : (
        <ul className="max-h-[520px] divide-y divide-line overflow-y-auto">
          {[...list.data].reverse().map((r) => (
            <li key={r.start_ms} className="flex items-center gap-3 px-4 py-2 text-sm">
              <span className="tabular w-32 shrink-0">
                {formatTime(r.start_ms, true)} → {formatTime(r.end_ms)}
              </span>
              <span className="tabular hidden text-muted sm:inline">{formatDuration(r.duration_ms)}</span>
              <span className="tabular hidden text-muted md:inline">{formatBytes(r.bytes)}</span>
              <div className="ml-auto flex gap-1">
                <Button size="sm" icon={<CirclePlay className="size-3.5" />} onClick={() => navigate(`/playback?camera=${encodeURIComponent(camera.id)}&t=${r.start_ms}`)}>
                  {t('Lire', 'Play')}
                </Button>
                {r.url ? (
                  <a href={r.url} download className="inline-flex h-8 items-center gap-1.5 rounded-md border border-line px-2.5 text-xs font-medium hover:bg-line/60">
                    <Download className="size-3.5" aria-hidden /> MP4
                  </a>
                ) : (
                  <Button size="sm" icon={<Download className="size-3.5" />} disabledReason={t('Enregistrement fictif (mode démo) : aucun fichier.', 'Fictional recording (demo mode): no file.')}>
                    MP4
                  </Button>
                )}
              </div>
            </li>
          ))}
        </ul>
      )}
    </Card>
  )
}

function TodayRecordedDuration({ cameraId }: { cameraId: string }) {
  const now = useNow(60_000)
  const list = useDayRecordings(cameraId, startOfDay(now))
  if (list.error) return <span className="text-muted">{t('indisponible', 'unavailable')}</span>
  if (!list.data) return <span className="text-muted">…</span>
  return <>{formatDuration(list.data.reduce((a, r) => a + r.duration_ms, 0))}</>
}

export function CameraDetailPage({ id }: { id: string }) {
  const camera = useLive((s) => s.cameras.find((c) => c.id === id))
  const loaded = useLive((s) => s.loaded)
  const error = useLive((s) => s.error)
  const ids = useLive((s) => s.cameras).map((c) => c.id)
  const demo = useIsDemo()
  const live = useCapability('live_video')
  const snapshots = useCapability('snapshots')
  const canManage = useCapability('camera_management') === true
  const readOnly = useReadOnly()
  const { source } = useData()
  const [toggling, setToggling] = useState(false)
  const [tab, setTab] = useState<TabKey>('live')
  const [editing, setEditing] = useState(false)
  const liveRef = useRef<HTMLDivElement>(null)
  const [liveStream, setLiveStream] = useState<LiveStream>('sub')
  const webrtc = useCapability('live_webrtc') === true
  const ptz = useCapability('ptz') === true
  const [transport, setTransport] = useState<'mse' | 'webrtc'>('mse')
  const tabs = useMemo(
    () => [
      { key: 'live' as const, label: t('Direct', 'Live'), icon: <Radio className="size-4" /> },
      { key: 'recordings' as const, label: t('Enregistrements', 'Recordings'), icon: <Video className="size-4" /> },
      { key: 'detection' as const, label: t('Détection', 'Detection'), icon: <Shapes className="size-4" /> },
      { key: 'config' as const, label: t('Configuration', 'Configuration'), icon: <Settings2 className="size-4" /> },
      { key: 'tech' as const, label: t('Informations techniques', 'Technical information'), icon: <Info className="size-4" /> },
    ],
    [],
  )

  if (!camera) {
    if (!loaded) return error ? <ErrorState error={error} /> : <LoadingState />
    return (
      <Card>
        <EmptyState title={t('Caméra introuvable', 'Camera not found')}>
          {t('Aucune caméra « ', 'No camera « ')}{id} ». <Link to="/cameras" className="text-accent hover:underline">{t('Retour à la liste', 'Back to the list')}</Link>
        </EmptyState>
      </Card>
    )
  }

  const noLive = demo ? t('Aucune vidéo à agrandir en mode démo.', 'No video to enlarge in demo mode.') : live ? (camera.status === 'online' ? null : t('Caméra hors ligne.', 'Camera offline.')) : t("Le direct vidéo n'est pas encore disponible.", "Live video is not available yet.")
  const tech: [string, ReactNode][] = [
    [t('Codec', 'Codec'), camera.codec?.toUpperCase() ?? '—'],
    [t('Résolution', 'Resolution'), formatResolution(camera.width, camera.height)],
    [t('Images par seconde (mesurées)', 'Frames per second (measured)'), camera.fps === null ? '—' : formatNumber(camera.fps, 1)],
    [t('Débit reçu', 'Received bitrate'), formatBitrate(camera.bitrate_kbps)],
    [t('Son', 'Sound'), camera.audio ? `${camera.audio}${camera.audio.startsWith('opus') ? t(' (G.711 de la caméra converti)', ' (G.711 of the camera converted)') : ''}, ${camera.record_audio ? t('enregistré', 'recorded') : t('non enregistré', 'not recorded')}` : camera.record_audio ? t('aucun son AAC ou G.711 reçu', 'no AAC or G.711 sound received') : t('non enregistré', 'not recorded')],
    [t('En ligne depuis', 'Online for'), camera.online_since_ms ? formatDateTime(camera.online_since_ms) : '—'],
    [t('Durée enregistrée aujourd’hui', 'Recorded today'), <TodayRecordedDuration cameraId={camera.id} />],
  ]

  return (
    <>
      <div className="mb-4">
        <Link to="/cameras" className="inline-flex items-center gap-1 text-xs text-muted hover:text-fg">
          <ArrowLeft className="size-3.5" aria-hidden />{t(' Caméras', ' Cameras')}
        </Link>
      </div>
      <div className="mb-4 flex flex-wrap items-start justify-between gap-3">
        <div>
          <div className="flex items-center gap-3">
            <h1 className="text-xl font-semibold">{camera.name}</h1>
            {demo && <Badge tone="demo">{t('Démo', 'Demo')}</Badge>}
          </div>
          <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted">
            <CameraStatusBadge status={camera.status} demo={demo} />
            {!demo && <MotionStatus camera={camera} />}
            <span className="font-mono">{camera.endpoint}</span>
            {!demo && <RecordingBadge camera={camera} />}
            <span>{camera.recording_enabled ? `${camera.record_mode === 'motion' ? t('Enregistrement sur mouvement', 'Recording on motion') : t('Enregistrement continu', 'Continuous recording')} (${destinationsLabel(camera.record_destinations)})` : t('Enregistrement désactivé', 'Recording off')}</span>
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            icon={<Maximize className="size-4" />}
            disabledReason={noLive}
            onClick={() => {
              setTab('live')
              // The live tab renders on the next frame; the click still counts as a user gesture.
              requestAnimationFrame(() => void liveRef.current?.requestFullscreen().catch(() => undefined))
            }}
          >
            {t('Plein écran', 'Full screen')}
          </Button>
          <Button
            icon={<CameraIcon className="size-4" />}
            disabledReason={
              demo
                ? t('Indisponible en mode démo.', 'Not available in demo mode.')
                : !snapshots
                  ? t("Les captures nécessitent FFmpeg, qui n'a pas pu être lancé sur le serveur (voir live.ffmpeg).", "Snapshots need FFmpeg, which could not be started on the server (see live.ffmpeg).")
                  : camera.status !== 'online'
                    ? t('Caméra hors ligne.', 'Camera offline.')
                    : null
            }
            onClick={() => {
              // Full-resolution picture of the main stream, downloaded as a file.
              const a = document.createElement('a')
              a.href = `${snapshotUrl(camera.id)}&stream=main`
              a.download = `${camera.id}-${new Date().toISOString().replace(/[:.]/g, '-')}.jpg`
              a.click()
            }}
          >
            {t('Capture', 'Snapshot')}
          </Button>
          <Button
            disabledReason={recordingToggleBlocker(camera, canManage, demo, readOnly) ?? (toggling ? t('Modification en cours…', 'Changing…') : null)}
            onClick={async () => {
              setToggling(true)
              try {
                source.store.upsertCamera(await setRecording(camera, !camera.recording_enabled))
              } catch (err) {
                source.store.notify('error', `${camera.name} : ${describeError(err)}`, camera.id)
              } finally {
                setToggling(false)
              }
            }}
          >
            {camera.recording_enabled ? t("Désactiver l'enregistrement", "Turn recording off") : t("Activer l'enregistrement", "Turn recording on")}
          </Button>
          <Button icon={<Shapes className="size-4" />} onClick={() => setTab('detection')}>
            {t('Zones de mouvement', 'Motion zones')}
          </Button>
          <Button variant="primary" icon={<CirclePlay className="size-4" />} onClick={() => navigate(`/playback?camera=${encodeURIComponent(camera.id)}`)}>
            {t('Lecture', 'Playback')}
          </Button>
        </div>
      </div>

      <Tabs tabs={tabs} active={tab} onChange={setTab} />
      <div className="mt-4">
        {tab === 'live' && (
          <div className="grid gap-4 xl:grid-cols-[minmax(0,1fr)_320px]">
            <div className="space-y-2">
              {!demo && (camera.has_sub_stream || webrtc) && (
                <div className="flex flex-wrap items-center justify-end gap-2 text-xs text-muted">
                  {webrtc && (
                    <>
                      {t('Mode', 'Mode')}
                      <Segmented
                        label={t('Mode du direct', 'Live view mode')}
                        options={[
                          { value: 'mse', label: t('Standard', 'Standard') },
                          { value: 'webrtc', label: t('Faible latence (WebRTC, expérimental)', 'Low latency (WebRTC, experimental)') },
                        ]}
                        value={transport}
                        onChange={setTransport}
                      />
                    </>
                  )}
                  {camera.has_sub_stream && (
                    <>
                  {t('Flux du direct', 'Live stream')}
                  <Segmented
                    label={t('Flux du direct', 'Live stream')}
                    options={[
                      { value: 'sub', label: t('Sous-flux', 'Sub stream') },
                      { value: 'main', label: t('Principal (HD)', 'Main (HD)') },
                    ]}
                    value={liveStream}
                    onChange={setLiveStream}
                  />
                    </>
                  )}
                </div>
              )}
              <div ref={liveRef} className="card aspect-video overflow-hidden">
                <CameraPlayer camera={camera} active large stream={camera.has_sub_stream ? liveStream : undefined} lowLatency={webrtc && transport === 'webrtc'} />
              </div>
            </div>
            <div className="space-y-4">
              {ptz && camera.ptz && <PtzPanel camera={camera} />}
              <Card>
                <CardHeader title={t('Flux', 'Streams')} />
                <div className="px-4 py-2">
                  <InfoList items={tech} />
                </div>
              </Card>
            </div>
          </div>
        )}
        {tab === 'recordings' && <RecordingsTab camera={camera} />}
        {tab === 'detection' && (
          <Card>
            <CardHeader title={t('Détection de mouvement', 'Motion detection')} subtitle={t("Différence d'images en niveaux de gris, zones en coordonnées relatives à l'image", "Greyscale picture difference, zones in picture-relative coordinates")} />
            <div className="p-4">
              <MotionPanel camera={camera} />
            </div>
          </Card>
        )}
        {tab === 'config' && (
          <Card>
            <CardHeader
              title={t('Configuration', 'Configuration')}
              subtitle={t('Lue depuis config.toml au démarrage', 'Read from config.toml at startup')}
              actions={
                <Button size="sm" icon={<Pencil className="size-3.5" />} onClick={() => setEditing(true)}>
                  {t('Modifier', 'Edit')}
                </Button>
              }
            />
            <div className="px-4 py-2">
              <InfoList
                items={[
                  [t('Identifiant', 'Username'), camera.id],
                  [t('Nom', 'Name'), camera.name],
                  [t('Activée', 'On'), camera.enabled ? t('Oui', 'Yes') : t('Non', 'No')],
                  [t('Flux principal', 'Main stream'), camera.endpoint],
                  [t('Sous-flux', 'Sub stream'), camera.has_sub_stream ? t(`Connecté pour le direct (${camera.sub_status ?? '—'})`, `Connected for the live view (${camera.sub_status ?? '—'})`) : t('Aucun', 'None')],
                  [t('Enregistrement continu', 'Continuous recording'), camera.recording_enabled ? t('Oui', 'Yes') : t('Non', 'No')],
                ]}
              />
              <p className="py-3 text-xs text-muted">{t("Le chemin RTSP et les identifiants ne sont jamais renvoyés par l'API.", "The RTSP path and the credentials are never returned by the API.")}</p>
            </div>
          </Card>
        )}
        {tab === 'tech' && (
          <div className="grid gap-4 lg:grid-cols-2">
            <Card>
              <CardHeader title={t('Ingestion RTSP', 'RTSP ingestion')} subtitle={t('Depuis le démarrage de RustNVR', 'Since RustNVR started')} />
              <div className="px-4 py-2">
                <InfoList
                  items={[
                    ...tech.slice(0, 5),
                    [t('Images reçues', 'Frames received'), formatNumber(camera.stream.frames)],
                    [t('Images clés', 'Key frames'), formatNumber(camera.stream.key_frames)],
                    [t('Données reçues', 'Data received'), formatBytes(camera.stream.bytes)],
                    [t('Paquets RTP perdus', 'RTP packets lost'), formatNumber(camera.stream.lost_packets)],
                    [t('Sessions', 'Sessions'), formatNumber(camera.stream.sessions)],
                    [t('Échecs / pertes de session', 'Session failures / losses'), formatNumber(camera.stream.failures)],
                    [t('Images abandonnées (file pleine)', 'Frames dropped (queue full)'), formatNumber(camera.stream.dropped_frames)],
                    [t('Dernière erreur', 'Last error'), camera.last_error ?? '—'],
                  ]}
                />
              </div>
            </Card>
            <Card>
              <CardHeader title={t('Enregistreur', 'Recorder')} subtitle={t('Depuis le démarrage de RustNVR', 'Since RustNVR started')} />
              <div className="px-4 py-2">
                {camera.recorder ? (
                  <InfoList
                    items={[
                      [t('Segments terminés', 'Finished segments'), formatNumber(camera.recorder.segments)],
                      [t('Images écrites', 'Frames written'), formatNumber(camera.recorder.frames_written)],
                      [t('Octets écrits', 'Bytes written'), formatBytes(camera.recorder.bytes_written)],
                      [t('Images ignorées', 'Frames skipped'), formatNumber(camera.recorder.frames_skipped)],
                      [t("Erreurs d'écriture", "Write errors"), formatNumber(camera.recorder.write_errors)],
                      [t('Discontinuités', 'Discontinuities'), formatNumber(camera.recorder.discontinuities)],
                    ]}
                  />
                ) : (
                  <EmptyState title={t('Enregistrement désactivé pour cette caméra', 'Recording off for this camera')} />
                )}
              </div>
            </Card>
          </div>
        )}
      </div>
      <CameraForm open={editing} camera={camera} existingIds={ids} onClose={() => setEditing(false)} />
    </>
  )
}
