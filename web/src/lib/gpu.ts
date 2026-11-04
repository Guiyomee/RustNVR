// Graphics cards reported by the server (GET /api/v1/system → gpus).

import type { Gpu } from '../types/api'
import { t } from './i18n'

/** Cards that can compute (not the display-only adapters of a VM or a server). */
export const computeGpus = (gpus: Gpu[] | undefined): Gpu[] => (gpus ?? []).filter((g) => g.status !== 'display_only')

/** Short state of a card. */
export function gpuStatusLabel(g: Gpu): string {
  switch (g.status) {
    case 'usable':
      return t('Utilisable par RustNVR', 'Usable by RustNVR')
    case 'not_passed':
      return t('Non transmise au conteneur', 'Not given to the container')
    case 'no_permission':
      return t('Accès refusé', 'Access denied')
    default:
      return t('Affichage seulement', 'Display only')
  }
}

/** What to do about a card that cannot be used. */
export function gpuStatusHelp(g: Gpu): string | null {
  switch (g.status) {
    case 'not_passed':
      return g.vendor === 'nvidia'
        ? t('Sous Docker : installez nvidia-container-toolkit sur l’hôte, ajoutez « gpus: all » au service dans compose.yaml et la variable NVIDIA_DRIVER_CAPABILITIES=compute,video,utility (« video » : conversions NVENC).', 'Under Docker: install nvidia-container-toolkit on the host, add « gpus: all » to the service in compose.yaml and the variable NVIDIA_DRIVER_CAPABILITIES=compute,video,utility (« video »: NVENC conversions).')
        : t(`Sous Docker : ajoutez « devices: [${g.device ?? '/dev/dri'}] » au service dans compose.yaml.`, `Under Docker: add « devices: [${g.device ?? '/dev/dri'}] » to the service in compose.yaml.`)
    case 'no_permission':
      return t(`Le compte de RustNVR ne peut pas ouvrir ${g.device ?? 'la carte'} : sous Docker, ajoutez « group_add » avec le groupe propriétaire (render ou video, voir « ls -n ${g.device ?? '/dev/dri'} » sur l’hôte).`, `RustNVR's account cannot open ${g.device ?? 'the card'}: under Docker, add « group_add » with the owning group (render or video, see « ls -n ${g.device ?? '/dev/dri'} » on the host).`)
    case 'display_only':
      return t('Carte virtuelle de la VM (ou puce d’affichage d’un serveur) : aucun calcul possible. Pour une vraie carte, transmettez-la à la VM (passthrough).', 'Virtual card of the VM (or display chip of a server): no computing possible. For a real card, pass it to the VM (passthrough).')
    default:
      return null
  }
}

/** Busiest card that can compute, when its driver tells. */
export function busiestGpu(gpus: Gpu[] | undefined): Gpu | null {
  const known = computeGpus(gpus).filter((g) => g.busy_percent !== null)
  return known.reduce<Gpu | null>((best, g) => (best === null || (g.busy_percent ?? 0) > (best.busy_percent ?? 0) ? g : best), null)
}
