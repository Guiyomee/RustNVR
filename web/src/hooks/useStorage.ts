import { useData } from '../data/DataContext'
import { useAsync, useInterval, type AsyncState } from './useAsync'
import type { Destination, Disk, Storage } from '../types/api'

/** Storage usage, refreshed every minute (the server caches it for 30 s). */
export function useStorage(): AsyncState<Storage> {
  const { source } = useData()
  const state = useAsync(() => source.storage(), [source])
  useInterval(state.reload, 60_000)
  return state
}

export function storagePercent(s: Storage | null): number | null {
  return diskPercent(s?.disk)
}

export function diskPercent(disk: Disk | null | undefined): number | null {
  if (!disk || disk.total_bytes === 0) return null
  return (100 * disk.used_bytes) / disk.total_bytes
}

/** S3 / SFTP: their disk is only the local outbox. */
export const isRemote = (d: Destination) => d.kind === 's3' || d.kind === 'sftp'

/** The destinations, the default one first. */
export function destinationsDefaultFirst(s: Storage): Destination[] {
  const def = s.destinations.find((d) => d.id === s.default_destination)
  return def ? [def, ...s.destinations.filter((d) => d !== def)] : s.destinations
}
