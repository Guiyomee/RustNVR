import { Pause, Play } from 'lucide-react'
import { useState } from 'react'
import { describeError } from '../../api/client'
import { setCameraPaused } from '../../api/endpoints'
import { useReadOnly } from '../../auth/AuthGate'
import { useCapability, useData, useIsDemo } from '../../data/DataContext'
import { t } from '../../lib/i18n'
import type { Camera } from '../../types/api'
import { Button, IconButton } from '../ui/Button'

/** Why the camera cannot be paused or resumed from here, or null. */
function blocker(camera: Camera, canManage: boolean, demo: boolean, readOnly: string | null): string | null {
  if (demo) return t('Indisponible en mode démo.', 'Not available in demo mode.')
  if (readOnly) return readOnly
  if (!canManage) return t('Ce serveur ne permet pas de piloter les caméras.', 'This server cannot drive the cameras.')
  if (!camera.enabled) return t('Caméra désactivée dans sa configuration.', 'Camera turned off in its configuration.')
  return null
}

/**
 * Pauses the camera (no connection, recording, detection or live view)
 * until it is resumed or RustNVR restarts; `compact`: an icon only.
 */
export function PauseButton({ camera, compact = false }: { camera: Camera; compact?: boolean }) {
  const demo = useIsDemo()
  const canManage = useCapability('camera_management') === true
  const readOnly = useReadOnly()
  const { source } = useData()
  const [busy, setBusy] = useState(false)
  const label = camera.paused
    ? t(`Reprendre ${camera.name}`, `Resume ${camera.name}`)
    : t(`Mettre ${camera.name} en pause jusqu’au redémarrage`, `Pause ${camera.name} until restart`)
  const reason = blocker(camera, canManage, demo, readOnly) ?? (busy ? t('Modification en cours…', 'Changing…') : null)
  const toggle = async () => {
    setBusy(true)
    try {
      source.store.upsertCamera(await setCameraPaused(camera.id, !camera.paused))
    } catch (e) {
      source.store.notify('error', `${camera.name} : ${describeError(e)}`, camera.id)
    } finally {
      setBusy(false)
    }
  }
  const icon = camera.paused ? <Play className="size-4" /> : <Pause className="size-4" />
  if (compact) return <IconButton label={label} icon={icon} disabledReason={reason} onClick={() => void toggle()} />
  return (
    <Button icon={icon} title={label} disabledReason={reason} onClick={() => void toggle()}>
      {camera.paused ? t('Reprendre', 'Resume') : t('Mettre en pause', 'Pause')}
    </Button>
  )
}
