import { CircleStop, Mic, MicOff, Pencil, PlugZap, Plus, Radar, Radio, Trash, Video } from 'lucide-react'
import { useState } from 'react'
import { CameraForm, type CameraPreset } from '../components/cameras/CameraForm'
import { DiscoveryDialog } from '../components/cameras/DiscoveryDialog'
import { RecordingBadge } from '../components/cameras/RecordingBadge'
import { Button, IconButton } from '../components/ui/Button'
import { Badge, Card, PageHeader } from '../components/ui/Card'
import { ConfirmDialog, Modal } from '../components/ui/Modal'
import { EmptyState, ErrorState, LoadingState } from '../components/ui/States'
import { CameraStatusBadge } from '../components/ui/StatusIndicator'
import { Switch } from '../components/ui/Switch'
import { useReadOnly } from '../auth/AuthGate'
import { describeError } from '../api/client'
import { deleteCamera, probeCamera, setAudio } from '../api/endpoints'
import { useCapability, useData, useIsDemo, useLive } from '../data/DataContext'
import { destinationsLabel, formatBitrate, formatDuration, formatNumber, formatResolution } from '../lib/format'
import { Link } from '../lib/router'
import { recordingToggleBlocker, setRecording } from '../lib/recording'
import type { Camera, ProbeResult } from '../types/api'
import { t } from '../lib/i18n'

function rtspState(c: Camera): string {
  switch (c.status) {
    case 'online':
      return c.online_since_ms ? t(`Session active depuis ${formatDuration(Date.now() - c.online_since_ms)}`, `Session active for ${formatDuration(Date.now() - c.online_since_ms)}`) : t('Session active', 'Session active')
    case 'connecting':
      return t(`Tentative ${c.attempt ?? 1} en cours`, `Attempt ${c.attempt ?? 1} in progress`)
    case 'retrying':
      return t(`Échec (tentative ${c.attempt ?? '?'}) · nouvel essai dans ${formatDuration(c.retry_in_ms)}`, `Failed (attempt ${c.attempt ?? '?'}) · retry in ${formatDuration(c.retry_in_ms)}`)
    case 'disabled':
      return t('Non connectée', 'Not connected')
    default:
      return '—'
  }
}

type Dialog =
  | { kind: 'none' }
  | { kind: 'form'; camera: Camera | null; preset?: CameraPreset }
  | { kind: 'discover' }
  | { kind: 'probe'; camera: Camera; state: 'running' | { ok: ProbeResult } | { error: string } }
  | { kind: 'delete'; camera: Camera; error: string | null; busy: boolean }

function Actions({ camera, onDialog }: { camera: Camera; onDialog: (d: Dialog) => void }) {
  const demo = useIsDemo()
  const manage = useCapability('camera_management') === true
  const readOnly = useReadOnly()
  const probe = async () => {
    onDialog({ kind: 'probe', camera, state: 'running' })
    try {
      onDialog({ kind: 'probe', camera, state: { ok: await probeCamera(camera.id) } })
    } catch (e) {
      onDialog({ kind: 'probe', camera, state: { error: describeError(e) } })
    }
  }
  const fromFile = t('Caméra déclarée dans config.toml : retirez sa section [[cameras]] puis redémarrez RustNVR.', 'Camera declared in config.toml: remove its [[cameras]] section then restart RustNVR.')
  return (
    <div className="flex justify-end gap-1">
      <IconButton
        label={t('Tester la connexion', 'Test the connection')}
        icon={<PlugZap className="size-4" />}
        disabledReason={demo ? t('Indisponible en mode démo.', 'Not available in demo mode.') : readOnly ? readOnly : manage ? null : t(`Utilisez « rustnvr probe --camera ${camera.id} ».`, `Use « rustnvr probe --camera ${camera.id} ».`)}
        onClick={() => void probe()}
      />
      <IconButton label={t('Modifier', 'Edit')} icon={<Pencil className="size-4" />} onClick={() => onDialog({ kind: 'form', camera })} />
      <IconButton
        label={t('Supprimer', 'Delete')}
        icon={<Trash className="size-4" />}
        disabledReason={demo ? t('Indisponible en mode démo.', 'Not available in demo mode.') : readOnly ? readOnly : !manage ? fromFile : camera.managed ? null : fromFile}
        onClick={() => onDialog({ kind: 'delete', camera, error: null, busy: false })}
      />
    </div>
  )
}

/** Recording on/off for one camera. */
function RecordingToggle({ camera }: { camera: Camera }) {
  const demo = useIsDemo()
  const canManage = useCapability('camera_management') === true
  const readOnly = useReadOnly()
  const { source } = useData()
  const [busy, setBusy] = useState(false)
  return (
    <Switch
      label={camera.recording_enabled ? t(`Arrêter l'enregistrement de ${camera.name}`, `Stop recording ${camera.name}`) : t(`Enregistrer ${camera.name}`, `Record ${camera.name}`)}
      checked={camera.recording_enabled}
      busy={busy}
      disabledReason={recordingToggleBlocker(camera, canManage, demo, readOnly)}
      onChange={async (record) => {
        setBusy(true)
        try {
          source.store.upsertCamera(await setRecording(camera, record))
        } catch (e) {
          source.store.notify('error', `${camera.name} : ${describeError(e)}`, camera.id)
        } finally {
          setBusy(false)
        }
      }}
    />
  )
}

/** Sound recording on/off for one camera. */
function AudioToggle({ camera }: { camera: Camera }) {
  const demo = useIsDemo()
  const canManage = useCapability('camera_management') === true
  const readOnly = useReadOnly()
  const { source } = useData()
  const [busy, setBusy] = useState(false)
  const label = camera.record_audio ? t(`Ne plus enregistrer le son de ${camera.name}`, `Stop recording the sound of ${camera.name}`) : t(`Enregistrer le son de ${camera.name}`, `Record the sound of ${camera.name}`)
  return (
    <IconButton
      label={label}
      icon={camera.record_audio ? <Mic className="size-4" /> : <MicOff className="size-4 text-muted" />}
      disabledReason={recordingToggleBlocker(camera, canManage, demo, readOnly) ?? (busy ? t('Modification en cours…', 'Changing…') : null)}
      onClick={async () => {
        setBusy(true)
        try {
          source.store.upsertCamera(await setAudio(camera.id, !camera.record_audio))
        } catch (e) {
          source.store.notify('error', `${camera.name} : ${describeError(e)}`, camera.id)
        } finally {
          setBusy(false)
        }
      }}
    />
  )
}

/** What the camera's sound looks like, for the recording column. */
function audioText(c: Camera): string {
  if (!c.record_audio) return t('Son non enregistré', 'Sound not recorded')
  const kind = (a: string) => (a.startsWith('mp4a') ? 'AAC' : a.startsWith('opus') ? t('G.711 converti en Opus', 'G.711 converted to Opus') : a)
  if (c.audio) return t(`Son enregistré (${kind(c.audio)})`, `Sound recorded (${kind(c.audio)})`)
  return c.status === 'online' ? t('Pas de son AAC ou G.711 reçu de la caméra', 'No AAC or G.711 sound received from the camera') : t('Son enregistré si la caméra en envoie', 'Sound recorded if the camera sends some')
}

/** Starts or stops the recording of every camera managed from the interface. */
function RecordAll({ cameras }: { cameras: Camera[] }) {
  const demo = useIsDemo()
  const canManage = useCapability('camera_management') === true
  const readOnly = useReadOnly()
  const { source } = useData()
  const [busy, setBusy] = useState(false)
  const [confirmStop, setConfirmStop] = useState(false)
  const switchable = cameras.filter((c) => recordingToggleBlocker(c, canManage, demo, readOnly) === null)
  const fixed = cameras.length - switchable.length
  const allOn = switchable.length > 0 && switchable.every((c) => c.recording_enabled)
  const apply = async (record: boolean) => {
    setBusy(true)
    let failed = 0
    for (const c of switchable.filter((c) => c.recording_enabled !== record)) {
      try {
        source.store.upsertCamera(await setRecording(c, record))
      } catch (e) {
        failed++
        source.store.notify('error', `${c.name} : ${describeError(e)}`, c.id)
      }
    }
    setBusy(false)
    const note = fixed > 0 ? t(` ${fixed} caméra(s) de config.toml inchangée(s).`, ` ${fixed} config.toml camera(s) unchanged.`) : ''
    if (failed === 0) source.store.notify('info', t(`${record ? 'Enregistrement activé' : 'Enregistrement arrêté'} pour toutes les caméras de l'interface.${note}`, `${record ? 'Recording on' : 'Recording stopped'} for every camera of the interface.${note}`))
  }
  const reason = demo
    ? t('Indisponible en mode démo.', 'Not available in demo mode.')
    : readOnly
      ? readOnly
      : switchable.length === 0
      ? t('Aucune caméra modifiable depuis l’interface (caméras de config.toml).', 'No camera can be changed from the interface (config.toml cameras).')
      : busy
        ? t('Modification en cours…', 'Changing…')
        : null
  return (
    <>
      {allOn ? (
        <Button icon={<CircleStop className="size-4" />} disabledReason={reason} onClick={() => setConfirmStop(true)}>
          {t('Tout arrêter', 'Stop all')}
        </Button>
      ) : (
        <Button icon={<Radio className="size-4" />} disabledReason={reason} onClick={() => void apply(true)}>
          {t('Tout enregistrer', 'Record all')}
        </Button>
      )}
      <ConfirmDialog
        open={confirmStop}
        title={t('Arrêter tous les enregistrements ?', 'Stop all recordings?')}
        danger
        confirmLabel={t('Tout arrêter', 'Stop all')}
        message={
          <>
            {t('Plus aucune des ', 'None of the ')}{switchable.length}{t(" caméra(s) gérées depuis l'interface ne sera enregistrée, jusqu'à ce que vous réactiviez l'enregistrement. Les vidéos déjà enregistrées sont conservées.", " camera(s) managed from the interface will be recorded, until you turn recording back on. The videos already recorded are kept.")}
            {fixed > 0 && <span className="mt-2 block">{fixed}{t(' caméra(s) de config.toml ne sont pas concernées.', ' config.toml camera(s) are not concerned.')}</span>}
          </>
        }
        onCancel={() => setConfirmStop(false)}
        onConfirm={() => {
          setConfirmStop(false)
          void apply(false)
        }}
      />
    </>
  )
}

function Origin({ camera }: { camera: Camera }) {
  return camera.managed ? <Badge tone="info">{t('Interface', 'Interface')}</Badge> : <Badge>config.toml</Badge>
}

export function CamerasPage() {
  const cameras = useLive((s) => s.cameras)
  const readOnly = useReadOnly()
  const loaded = useLive((s) => s.loaded)
  const error = useLive((s) => s.error)
  const demo = useIsDemo()
  const { source } = useData()
  const [dialog, setDialog] = useState<Dialog>({ kind: 'none' })
  const discovery = useCapability('discovery') === true
  const ids = cameras.map((c) => c.id)
  const remove = async (camera: Camera) => {
    setDialog({ kind: 'delete', camera, error: null, busy: true })
    try {
      await deleteCamera(camera.id)
      source.store.removeCamera(camera.id)
      source.store.notify('info', t(`${camera.name} : supprimée (enregistrements conservés)`, `${camera.name}: deleted (recordings kept)`), camera.id)
      setDialog({ kind: 'none' })
    } catch (e) {
      setDialog({ kind: 'delete', camera, error: describeError(e), busy: false })
    }
  }

  return (
    <>
      <PageHeader
        title={t('Caméras', 'Cameras')}
        description={t("Caméras de config.toml et caméras ajoutées depuis l'interface, avec l'état de leur connexion RTSP.", "Cameras of config.toml and cameras added from the interface, with the state of their RTSP connection.")}
        actions={
          <div className="flex flex-wrap gap-2">
            {loaded && cameras.length > 0 && <RecordAll cameras={cameras} />}
            <Button
              icon={<Radar className="size-4" />}
              disabledReason={readOnly ?? (demo ? t('Indisponible en mode démo.', 'Not available in demo mode.') : discovery ? null : t('Ce serveur ne sait pas rechercher les caméras (version plus ancienne de RustNVR).', 'This server cannot find cameras (older RustNVR version).'))}
              onClick={() => setDialog({ kind: 'discover' })}
            >
              {t('Rechercher sur le réseau', 'Find on the network')}
            </Button>
            <Button variant="primary" icon={<Plus className="size-4" />} disabledReason={readOnly} onClick={() => setDialog({ kind: 'form', camera: null })}>
              {t('Ajouter une caméra', 'Add a camera')}
            </Button>
          </div>
        }
      />
      {!loaded ? (
        error ? <ErrorState error={error} /> : <LoadingState />
      ) : cameras.length === 0 ? (
        <Card>
          <EmptyState title={t('Aucune caméra configurée', 'No camera configured')} icon={<Video className="size-7" />}>
            {t('Utilisez « Ajouter une caméra » pour générer la section à placer dans config.toml.', 'Use « Add a camera » to generate the section to put in config.toml.')}
          </EmptyState>
        </Card>
      ) : (
        <>
          {/* Table on large screens */}
          <Card className="hidden overflow-x-auto lg:block">
            <table className="w-full text-sm">
              <thead className="border-b border-line text-left text-xs text-muted">
                <tr>
                  <th className="px-4 py-3 font-medium">{t('Caméra', 'Camera')}</th>
                  <th className="px-4 py-3 font-medium">{t('État RTSP', 'RTSP state')}</th>
                  <th className="px-4 py-3 font-medium">{t('Flux', 'Streams')}</th>
                  <th className="px-4 py-3 font-medium">{t('Enregistrement', 'Recording')}</th>
                  <th className="px-4 py-3 font-medium">{t('Adresse', 'Address')}</th>
                  <th className="px-4 py-3" />
                </tr>
              </thead>
              <tbody className="divide-y divide-line">
                {cameras.map((c) => (
                  <tr key={c.id} className="align-top hover:bg-line/20">
                    <td className="px-4 py-3">
                      <Link to={`/cameras/${encodeURIComponent(c.id)}`} className="font-medium hover:text-accent">
                        {c.name}
                      </Link>
                      <div className="mt-0.5 flex items-center gap-2 text-xs text-muted">
                        {c.id} <Origin camera={c} />
                      </div>
                    </td>
                    <td className="px-4 py-3">
                      <CameraStatusBadge status={c.status} demo={demo} />
                      <div className="mt-0.5 text-xs text-muted">{rtspState(c)}</div>
                      {c.last_error && <div className="mt-0.5 max-w-xs truncate text-xs text-err" title={c.last_error}>{c.last_error}</div>}
                    </td>
                    <td className="tabular px-4 py-3 text-xs">
                      <div>
                        {c.codec?.toUpperCase() ?? '—'} · {formatResolution(c.width, c.height)}
                      </div>
                      <div className="text-muted">
                        {c.fps === null ? '—' : t(`${formatNumber(c.fps, 1)} i/s`, `${formatNumber(c.fps, 1)} fps`)} · {formatBitrate(c.bitrate_kbps)}
                      </div>
                      <div className="text-muted">{t('Sous-flux : ', 'Sub stream: ')}{c.has_sub_stream ? (c.sub_status === 'online' ? t('en ligne (direct)', 'online (live view)') : t(`direct, ${c.sub_status ?? '—'}`, `live view, ${c.sub_status ?? '—'}`)) : 'aucun'}</div>
                    </td>
                    <td className="px-4 py-3 text-xs">
                      <div className="mb-1 flex items-center gap-2">
                        <RecordingToggle camera={c} />
                        <RecordingBadge camera={c} />
                        <AudioToggle camera={c} />
                      </div>
                      <div className="text-muted">{audioText(c)}</div>
                      <div className="text-muted">{c.recording_enabled ? `${c.record_mode === 'motion' ? t('Sur mouvement', 'On motion') : t('Continu', 'Continuous')} · ${destinationsLabel(c.record_destinations)}` : t('Activez pour enregistrer', 'Turn on to record')}</div>
                      {c.recorder && (
                        <div className="text-muted">
                          {formatNumber(c.recorder.segments)}{t(' segments · ', ' segments · ')}{formatNumber(c.recorder.write_errors)}{t(' erreurs', ' errors')}
                        </div>
                      )}
                    </td>
                    <td className="px-4 py-3 font-mono text-xs text-muted">{c.endpoint}</td>
                    <td className="px-4 py-3">
                      <Actions camera={c} onDialog={setDialog} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </Card>

          {/* Cards on small screens */}
          <div className="grid gap-3 lg:hidden">
            {cameras.map((c) => (
              <Card key={c.id} className="space-y-2 p-4">
                <div className="flex items-start justify-between gap-2">
                  <div className="min-w-0">
                    <Link to={`/cameras/${encodeURIComponent(c.id)}`} className="font-medium hover:text-accent">
                      {c.name}
                    </Link>
                    <div className="text-xs text-muted">{c.endpoint}</div>
                  </div>
                  <CameraStatusBadge status={c.status} demo={demo} />
                </div>
                <p className="text-xs text-muted">{rtspState(c)}</p>
                <div className="flex items-center gap-2 text-xs">
                  <RecordingToggle camera={c} />
                  <RecordingBadge camera={c} />
                  <AudioToggle camera={c} />
                </div>
                <p className="tabular text-xs">
                  {c.codec?.toUpperCase() ?? '—'} · {formatResolution(c.width, c.height)} · {c.fps === null ? '—' : t(`${formatNumber(c.fps, 1)} i/s`, `${formatNumber(c.fps, 1)} fps`)}
                </p>
                <Actions camera={c} onDialog={setDialog} />
              </Card>
            ))}
          </div>
        </>
      )}
      <CameraForm open={dialog.kind === 'form'} camera={dialog.kind === 'form' ? dialog.camera : null} preset={dialog.kind === 'form' ? dialog.preset : null} existingIds={ids} onClose={() => setDialog({ kind: 'none' })} />
      <DiscoveryDialog open={dialog.kind === 'discover'} onClose={() => setDialog((d) => (d.kind === 'discover' ? { kind: 'none' } : d))} onChoose={(preset) => setDialog({ kind: 'form', camera: null, preset })} />
      <Modal
        open={dialog.kind === 'probe'}
        title={dialog.kind === 'probe' ? t(`Test de connexion : ${dialog.camera.name}`, `Connection test: ${dialog.camera.name}`) : ''}
        onClose={() => setDialog({ kind: 'none' })}
        footer={<Button onClick={() => setDialog({ kind: 'none' })}>{t('Fermer', 'Close')}</Button>}
      >
        {dialog.kind === 'probe' &&
          (dialog.state === 'running' ? (
            <p className="text-sm text-muted">{t("Connexion à la caméra… (jusqu'à 30 s)", "Connecting to the camera… (up to 30 s)")}</p>
          ) : 'error' in dialog.state ? (
            <p className="text-sm text-err">{t('Échec : ', 'Failed: ')}{dialog.state.error}</p>
          ) : (
            <div className="space-y-1 text-sm">
              {dialog.state.ok.video ? (
                <p className="text-ok">
                  {t('Connexion réussie : ', 'Connected: ')}{dialog.state.ok.video.codec.toUpperCase()} {formatResolution(dialog.state.ok.video.width, dialog.state.ok.video.height)}{t(', première image après ', ', first picture after ')}{dialog.state.ok.video.time_to_key_frame_ms}{t(' ms.', ' ms.')}
                </p>
              ) : (
                <p className="text-warn">{t("La caméra répond mais n'annonce aucun flux H.264/H.265.", "The camera answers but announces no H.264/H.265 stream.")}</p>
              )}
              <p className="text-xs text-muted">{t('Flux annoncés : ', 'Announced streams: ')}{dialog.state.ok.streams.map((x) => `${x.media}/${x.encoding}`).join(', ')}</p>
            </div>
          ))}
      </Modal>
      <ConfirmDialog
        open={dialog.kind === 'delete'}
        title={t('Supprimer la caméra ?', 'Delete the camera?')}
        danger
        confirmLabel={dialog.kind === 'delete' && dialog.busy ? t('Suppression…', 'Deleting…') : t('Supprimer', 'Delete')}
        message={
          dialog.kind === 'delete' ? (
            <>
              <strong className="text-fg">{dialog.camera.name}</strong>{t(' sera déconnectée et retirée. Ses enregistrements déjà faits restent sur le disque.', ' will be disconnected and removed. Its existing recordings stay on the disk.')}
              {dialog.error && <span className="mt-2 block text-err">{t('Échec : ', 'Failed: ')}{dialog.error}</span>}
            </>
          ) : null
        }
        onCancel={() => setDialog({ kind: 'none' })}
        onConfirm={() => dialog.kind === 'delete' && !dialog.busy && void remove(dialog.camera)}
      />
    </>
  )
}
