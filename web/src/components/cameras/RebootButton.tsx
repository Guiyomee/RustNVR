import { RotateCcw } from 'lucide-react'
import { useState } from 'react'
import { describeError } from '../../api/client'
import { rebootCamera } from '../../api/endpoints'
import { useReadOnly } from '../../auth/AuthGate'
import { useCapability, useData, useIsDemo } from '../../data/DataContext'
import { t } from '../../lib/i18n'
import type { Camera } from '../../types/api'
import { Button } from '../ui/Button'
import { ConfirmDialog } from '../ui/Modal'

/** Restarts the camera itself through ONVIF, for a camera whose ONVIF port is set. */
export function RebootButton({ camera }: { camera: Camera }) {
  const demo = useIsDemo()
  const canManage = useCapability('camera_management') === true
  const readOnly = useReadOnly()
  const { source } = useData()
  const [asking, setAsking] = useState(false)
  const [busy, setBusy] = useState(false)
  if (!camera.ptz) return null
  const reason = demo
    ? t('Indisponible en mode démo.', 'Not available in demo mode.')
    : (readOnly ?? (!canManage ? t('Ce serveur ne permet pas de piloter les caméras.', 'This server cannot drive the cameras.') : busy ? t('Redémarrage demandé…', 'Restart asked…') : null))
  const reboot = async () => {
    setAsking(false)
    setBusy(true)
    try {
      const { message } = await rebootCamera(camera.id)
      source.store.notify('info', t(`${camera.name} redémarre${message ? ` (${message})` : ''} : de retour dans une à deux minutes.`, `${camera.name} is restarting${message ? ` (${message})` : ''}: back in one or two minutes.`), camera.id)
    } catch (e) {
      source.store.notify('error', `${camera.name} : ${describeError(e)}`, camera.id)
    } finally {
      setBusy(false)
    }
  }
  return (
    <>
      <Button icon={<RotateCcw className="size-4" />} disabledReason={reason} onClick={() => setAsking(true)}>
        {t('Redémarrer la caméra', 'Restart the camera')}
      </Button>
      <ConfirmDialog
        open={asking}
        danger
        title={t('Redémarrer la caméra ?', 'Restart the camera?')}
        confirmLabel={t('Redémarrer', 'Restart')}
        message={t(
          `${camera.name} redémarre elle-même (par ONVIF) : pas d’image ni d’enregistrement pendant une à deux minutes. Il faut un compte administrateur de la caméra.`,
          `${camera.name} restarts itself (through ONVIF): no picture nor recording for one or two minutes. It needs an administrator account of the camera.`,
        )}
        onCancel={() => setAsking(false)}
        onConfirm={() => void reboot()}
      />
    </>
  )
}
