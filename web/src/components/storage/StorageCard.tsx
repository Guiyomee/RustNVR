import { HardDrive } from 'lucide-react'
import { destinationsDefaultFirst, diskPercent, isRemote, storagePercent } from '../../hooks/useStorage'
import { formatBytes, formatNumber } from '../../lib/format'
import type { Destination, Storage } from '../../types/api'
import { Badge, Card, CardHeader } from '../ui/Card'
import { Meter } from '../ui/Meter'
import { ErrorState, LoadingState } from '../ui/States'
import { t } from '../../lib/i18n'

const KIND: Record<Destination['kind'], string> = { local: t('Disque local', 'Local disk'), nas: 'NAS', s3: 'S3', sftp: 'SFTP' }

/** One destination: its disk (or its uploads, for S3 / SFTP). */
function DestinationRow({ dest, isDefault }: { dest: Destination; isDefault: boolean }) {
  const pct = diskPercent(dest.disk)
  const remote = isRemote(dest)
  return (
    <li className="space-y-1.5 py-3 first:pt-0 last:pb-0">
      <div className="flex items-center gap-2">
        <span className="min-w-0 truncate text-sm font-medium" title={dest.path}>
          {dest.name}
        </span>
        {isDefault && <Badge tone="info">{t('Par défaut', 'Default')}</Badge>}
        {!dest.available ? <Badge tone="err">{t('Indisponible', 'Unavailable')}</Badge> : dest.low_space && <Badge tone="warn">{t('Espace faible', 'Low space')}</Badge>}
        <span className="ml-auto shrink-0 text-xs text-muted">{KIND[dest.kind] !== dest.name && KIND[dest.kind]}</span>
        {!remote && <span className="tabular w-12 shrink-0 text-right text-sm font-semibold">{pct === null ? '—' : `${formatNumber(pct)} %`}</span>}
      </div>
      {!dest.available ? (
        <p className="text-xs text-err">{dest.unavailable_reason ?? t('Point de stockage indisponible', 'Destination unavailable')}</p>
      ) : remote ? (
        <p className="text-xs text-muted tabular">
          {dest.upload
            ? t(`${formatNumber(dest.upload.pending)} en attente d’envoi · ${formatBytes(dest.upload.uploaded_bytes)} envoyés`, `${formatNumber(dest.upload.pending)} waiting to upload · ${formatBytes(dest.upload.uploaded_bytes)} uploaded`)
            : t('Envoi des segments terminés', 'Finished segments uploaded')}
          {dest.upload?.error && <span className="block text-err">{dest.upload.error}</span>}
        </p>
      ) : (
        <>
          <Meter value={pct} label={t(`Occupation de ${dest.name}`, `${dest.name} usage`)} />
          <p className="flex flex-wrap justify-between gap-x-3 text-xs text-muted tabular">
            <span>{dest.disk ? t(`${formatBytes(dest.disk.free_bytes)} libres sur ${formatBytes(dest.disk.total_bytes)}`, `${formatBytes(dest.disk.free_bytes)} free of ${formatBytes(dest.disk.total_bytes)}`) : t('Taille inconnue', 'Unknown size')}</span>
            <span>
              {formatBytes(dest.recordings_bytes)}
              {t(' de vidéos', ' of videos')}
              {dest.estimated_days_until_full != null &&
                (dest.estimated_days_until_full < 1
                  ? t(' · plein en moins d’un jour', ' · full in less than a day')
                  : t(` · plein dans ≈ ${formatNumber(dest.estimated_days_until_full)} j`, ` · full in ≈ ${formatNumber(dest.estimated_days_until_full)} d`))}
            </span>
          </p>
        </>
      )}
    </li>
  )
}

/** Every destination with its disk, the default one first. */
export function DestinationDisks({ storage }: { storage: Storage }) {
  return (
    <div className="space-y-3">
      <ul className="divide-y divide-line">
        {destinationsDefaultFirst(storage).map((d) => (
          <DestinationRow key={d.id} dest={d} isDefault={d.id === storage.default_destination} />
        ))}
      </ul>
      {!storage.retention_configured && (
        <p className="rounded border border-warn/30 bg-warn/10 px-2.5 py-1.5 text-xs text-warn">
          {t('Aucune politique de rétention : les enregistrements ne sont jamais supprimés automatiquement.', 'No retention policy: recordings are never deleted automatically.')}
        </p>
      )}
    </div>
  )
}

export function StorageSummary({ storage }: { storage: Storage }) {
  const pct = storagePercent(storage)
  const d = storage.disk
  return (
    <div className="space-y-3">
      <div className="flex items-end justify-between">
        <div className="tabular text-2xl font-semibold">{pct === null ? '—' : `${formatNumber(pct)} %`}</div>
        <div className="text-right text-xs text-muted">{d ? `${formatBytes(d.used_bytes)} / ${formatBytes(d.total_bytes)}` : t('Disque inconnu', 'Unknown disk')}</div>
      </div>
      <Meter value={pct} label={t('Occupation du disque des enregistrements', 'Recordings disk usage')} />
      <dl className="grid grid-cols-2 gap-x-4 gap-y-2 text-sm">
        <div>
          <dt className="text-xs text-muted">{t('Libre', 'Free')}</dt>
          <dd className="tabular">{formatBytes(d?.free_bytes)}</dd>
        </div>
        <div>
          <dt className="text-xs text-muted">{t('Enregistrements', 'Recordings')}</dt>
          <dd className="tabular">{formatBytes(storage.recordings_bytes)}</dd>
        </div>
        <div className="col-span-2">
          <dt className="text-xs text-muted">{t('Autonomie estimée', 'Estimated time left')}</dt>
          <dd className="text-sm">
            {storage.estimated_days_until_full === null ? (
              <span className="text-muted">{t("Pas assez d'historique (≥ 1 h d'enregistrements requis)", "Not enough history (≥ 1 h of recordings needed)")}</span>
            ) : (
              <>
                <span className="tabular">≈ {formatNumber(storage.estimated_days_until_full)}{t(' jours', ' days')}</span>
                <span className="text-xs text-muted">{t(' avant disque plein au rythme actuel', ' before the disk is full at the current rate')}</span>
              </>
            )}
          </dd>
        </div>
      </dl>
      {!storage.retention_configured && (
        <p className="rounded border border-warn/30 bg-warn/10 px-2.5 py-1.5 text-xs text-warn">
          {t('Aucune politique de rétention : les enregistrements ne sont jamais supprimés automatiquement.', 'No retention policy: recordings are never deleted automatically.')}
        </p>
      )}
    </div>
  )
}

export function StorageCard({ storage, error, onRetry }: { storage: Storage | null; error: unknown; onRetry: () => void }) {
  return (
    <Card>
      <CardHeader title={t('Stockage', 'Storage')} icon={<HardDrive className="size-4" />} />
      <div className="p-4">
        {storage ? <DestinationDisks storage={storage} /> : error ? <ErrorState error={error} onRetry={onRetry} compact /> : <LoadingState compact />}
      </div>
    </Card>
  )
}
