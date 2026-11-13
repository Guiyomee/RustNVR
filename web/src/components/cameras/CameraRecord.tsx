import { RefreshCw } from 'lucide-react'
import { useState } from 'react'
import { describeError } from '../../api/client'
import { getCameraDevice, type CameraDevice } from '../../api/endpoints'
import { useIsDemo } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { t } from '../../lib/i18n'
import type { Camera } from '../../types/api'
import { IconButton } from '../ui/Button'
import { Card, CardHeader, InfoList } from '../ui/Card'
import { EmptyState, LoadingState } from '../ui/States'

/** Maker, model, firmware and serial number of the camera, read through ONVIF. */
export function CameraRecord({ camera }: { camera: Camera }) {
  const demo = useIsDemo()
  const [fresh, setFresh] = useState<CameraDevice | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const kept = useAsync(() => (demo || !camera.ptz ? Promise.resolve(null) : getCameraDevice(camera.id)), [demo, camera.id, camera.ptz])
  const reload = async () => {
    setBusy(true)
    setError(null)
    try {
      setFresh(await getCameraDevice(camera.id, true))
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }
  const device = fresh ?? kept.data
  const failure = error ?? (kept.error ? describeError(kept.error) : null)
  const value = (v: string | null | undefined) => v ?? t('non donné par la caméra', 'not given by the camera')
  return (
    <Card>
      <CardHeader
        title={t('Fiche de la caméra', 'Camera record')}
        subtitle={t('Ce que la caméra dit d’elle-même (ONVIF)', 'What the camera says about itself (ONVIF)')}
        actions={
          camera.ptz && !demo ? (
            <IconButton label={t('Relire la fiche', 'Read the record again')} icon={<RefreshCw className="size-4" />} disabledReason={busy ? t('Lecture en cours…', 'Reading…') : null} onClick={() => void reload()} />
          ) : undefined
        }
      />
      <div className="px-4 py-2">
        {demo ? (
          <EmptyState title={t('Indisponible en mode démo.', 'Not available in demo mode.')} />
        ) : !camera.ptz ? (
          <EmptyState title={t('Port ONVIF non réglé', 'ONVIF port not set')}>
            {t('Réglez le port ONVIF de la caméra (Modifier → Port ONVIF) pour lire son fabricant, son modèle, son firmware et son numéro de série.', 'Set the camera’s ONVIF port (Edit → ONVIF port) to read its maker, model, firmware and serial number.')}
          </EmptyState>
        ) : device ? (
          <InfoList
            items={[
              [t('Fabricant', 'Maker'), value(device.manufacturer)],
              [t('Modèle', 'Model'), value(device.model)],
              [t('Firmware', 'Firmware'), value(device.firmware)],
              [t('Numéro de série', 'Serial number'), value(device.serial)],
              [t('Matériel', 'Hardware'), value(device.hardware)],
            ]}
          />
        ) : failure ? (
          <p className="py-2 text-sm text-err">{failure}</p>
        ) : (
          <LoadingState compact />
        )}
        {device && error && <p className="pb-2 text-xs text-err">{error}</p>}
      </div>
    </Card>
  )
}
