import { updateCamera } from '../api/endpoints'
import type { Camera } from '../types/api'
import { t } from './i18n'

export type RecordingTone = 'rec' | 'idle' | 'warn' | 'off'

/** What the camera is doing right now regarding recording, as shown in the interface. */
export function recordingState(c: Camera): { tone: RecordingTone; label: string; title: string } {
  if (!c.enabled) return { tone: 'off', label: t('Désactivée', 'Off'), title: t('Caméra désactivée : rien n’est enregistré.', 'Camera off: nothing is recorded.') }
  if (!c.recording_enabled) return { tone: 'off', label: t('Non enregistrée', 'Not recorded'), title: t('Enregistrement désactivé pour cette caméra.', 'Recording off for this camera.') }
  if (c.status !== 'online') return { tone: 'warn', label: t('Interrompu', 'Interrupted'), title: t('Enregistrement activé mais caméra hors ligne : rien n’est écrit.', 'Recording on but camera offline: nothing is written.') }
  if ((c.recorder?.write_errors ?? 0) > 0) return { tone: 'warn', label: t('REC · erreurs', 'REC · errors'), title: t(`${c.recorder?.write_errors} erreur(s) d’écriture depuis le démarrage.`, `${c.recorder?.write_errors} write error(s) since startup.`) }
  if (c.record_mode === 'motion') {
    if (!c.motion.enabled) return { tone: 'warn', label: t('Détection coupée', 'Detection off'), title: t('Enregistrement sur mouvement mais détection désactivée : rien n’est enregistré.', 'Recording on motion but detection off: nothing is recorded.') }
    return c.motion.active
      ? { tone: 'rec', label: 'REC', title: t('Mouvement en cours : enregistrement.', 'Motion now: recording.') }
      : { tone: 'idle', label: t('Sur mouvement', 'On motion'), title: t('En attente d’un mouvement (images d’avant gardées en mémoire).', 'Waiting for motion (earlier pictures kept in memory).') }
  }
  return { tone: 'rec', label: 'REC', title: t('Enregistrement continu en cours.', 'Continuous recording in progress.') }
}

/** Why recording cannot be switched from the interface for `camera`, or null. */
export function recordingToggleBlocker(camera: Camera, canManage: boolean, demo: boolean, readOnly: string | null = null): string | null {
  if (demo) return t('Indisponible en mode démo.', 'Not available in demo mode.')
  if (readOnly) return readOnly
  if (!canManage || !camera.managed || !camera.edit) return t('Caméra déclarée dans config.toml : modifiez « record = { enabled = … } » dans ce fichier puis redémarrez RustNVR.', 'Camera declared in config.toml: change « record = { enabled = … } » in this file then restart RustNVR.')
  return null
}

/**
 * Enables or disables the recording of a camera managed from the interface.
 * The password is omitted (the server keeps the stored one); enabling a
 * camera that had no destination uses the default destination.
 */
export async function setRecording(camera: Camera, record: boolean): Promise<Camera> {
  const e = camera.edit
  if (!e) throw new Error(t('caméra non modifiable depuis l’interface', 'camera cannot be changed from the interface'))
  const stream = (url: string) => ({ url, username: e.username, transport: e.transport })
  return updateCamera(camera.id, {
    name: camera.name,
    enabled: camera.enabled,
    record,
    // Everything else as it is: the webcam, or the streams and the ONVIF port.
    ...(e.usb
      ? { main: null, sub: null, usb: e.usb, onvif_port: null }
      : { main: stream(e.main_url), sub: e.sub_url ? stream(e.sub_url) : null, usb: null, onvif_port: e.onvif_port ?? null }),
    record_destinations: camera.record_destinations,
    record_mode: camera.record_mode,
    retention_days: camera.retention_days,
    max_size_gb: camera.max_size_gb,
    record_audio: camera.record_audio,
  })
}
