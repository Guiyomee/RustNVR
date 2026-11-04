import { TriangleAlert, Construction, Inbox, LoaderCircle, LockKeyhole, RefreshCw } from 'lucide-react'
import type { ReactNode } from 'react'
import { ApiError, UnavailableError, describeError } from '../../api/client'
import { Button } from './Button'
import { t } from '../../lib/i18n'

export function LoadingState({ label = t('Chargement…', 'Loading…'), compact = false }: { label?: string; compact?: boolean }) {
  return (
    <div role="status" className={`flex items-center justify-center gap-2 text-muted ${compact ? 'py-4' : 'py-12'}`}>
      <LoaderCircle className="size-4 animate-spin" aria-hidden />
      <span>{label}</span>
    </div>
  )
}

export function EmptyState({ title, children, icon }: { title: string; children?: ReactNode; icon?: ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 px-4 py-10 text-center">
      <div className="text-muted">{icon ?? <Inbox className="size-7" aria-hidden />}</div>
      <p className="font-medium">{title}</p>
      {children && <div className="max-w-md text-sm text-muted">{children}</div>}
    </div>
  )
}

/**
 * A feature the backend does not implement yet. Shows what is missing and
 * where its planned contract is documented, instead of fake data.
 */
export function Unavailable({ title, children, route, compact = false }: { title: string; children?: ReactNode; route?: string; compact?: boolean }) {
  return (
    <div className={`flex flex-col items-center justify-center gap-2 text-center ${compact ? 'px-3 py-5' : 'px-4 py-10'}`}>
      <Construction className="size-6 text-muted" aria-hidden />
      <p className="font-medium">{title}</p>
      {children && <div className="max-w-md text-sm text-muted">{children}</div>}
      {route && (
        <p className="text-xs text-muted">
          {t('Route prévue : ', 'Planned route: ')}<code className="rounded bg-bg-2 px-1 py-0.5">{route}</code>{t(' (voir API_CONTRACT.md)', ' (see API_CONTRACT.md)')}
        </p>
      )}
    </div>
  )
}

export function ErrorState({ error, onRetry, compact = false }: { error: unknown; onRetry?: () => void; compact?: boolean }) {
  if (error instanceof UnavailableError) {
    return <Unavailable title={t('Fonctionnalité non disponible', 'Feature not available')} route={error.route} compact={compact}>{error.message}</Unavailable>
  }
  const auth = error instanceof ApiError && error.kind === 'auth'
  return (
    <div role="alert" className={`flex flex-col items-center justify-center gap-2 text-center ${compact ? 'py-5' : 'py-10'}`}>
      {auth ? <LockKeyhole className="size-6 text-warn" aria-hidden /> : <TriangleAlert className="size-6 text-err" aria-hidden />}
      <p className="font-medium">{auth ? t('Authentification requise', 'Sign-in required') : t('Impossible de charger les données', 'Cannot load the data')}</p>
      <p className="max-w-md text-sm text-muted">
        {auth ? t('Rechargez la page pour saisir vos identifiants.', 'Reload the page to enter your credentials.') : describeError(error)}
      </p>
      {onRetry && (
        <Button size="sm" onClick={onRetry} icon={<RefreshCw className="size-3.5" aria-hidden />}>
          {t('Réessayer', 'Retry')}
        </Button>
      )}
    </div>
  )
}
