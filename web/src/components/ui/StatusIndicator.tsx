import { CircleCheck, CircleDashed, CircleOff, LoaderCircle, CirclePause, CircleX } from 'lucide-react'
import type { CameraStatus } from '../../types/api'
import { t } from '../../lib/i18n'

export type Tone = 'ok' | 'warn' | 'err' | 'info' | 'neutral'

export const toneText: Record<Tone, string> = {
  ok: 'text-ok',
  warn: 'text-warn',
  err: 'text-err',
  info: 'text-accent',
  neutral: 'text-muted',
}

const toneDot: Record<Tone, string> = {
  ok: 'bg-ok',
  warn: 'bg-warn',
  err: 'bg-err',
  info: 'bg-accent',
  neutral: 'bg-muted',
}

export const cameraStatusInfo: Record<CameraStatus, { tone: Tone; label: string }> = {
  online: { tone: 'ok', label: t('En ligne', 'Online') },
  connecting: { tone: 'warn', label: t('Connexion…', 'Connecting…') },
  retrying: { tone: 'err', label: t('Hors ligne', 'Offline') },
  idle: { tone: 'neutral', label: t('En attente', 'Waiting') },
  stopped: { tone: 'neutral', label: t('Arrêtée', 'Stopped') },
  disabled: { tone: 'neutral', label: t('Désactivée', 'Off') },
}

const icons: Record<CameraStatus, typeof CircleCheck> = {
  online: CircleCheck,
  connecting: LoaderCircle,
  retrying: CircleX,
  idle: CircleDashed,
  stopped: CirclePause,
  disabled: CircleOff,
}

/** A dot (or icon) plus a label: state is never conveyed by color alone. */
export function StatusDot({ tone, label, pulse = false }: { tone: Tone; label: string; pulse?: boolean }) {
  return (
    <span className="inline-flex items-center gap-1.5 text-xs">
      <span className={`relative inline-flex size-2 rounded-full ${toneDot[tone]}`} aria-hidden>
        {pulse && <span className={`absolute inset-0 animate-ping rounded-full opacity-60 ${toneDot[tone]}`} />}
      </span>
      <span className={toneText[tone]}>{label}</span>
    </span>
  )
}

export function CameraStatusBadge({ status, demo = false }: { status: CameraStatus; demo?: boolean }) {
  const { tone, label } = cameraStatusInfo[status]
  const Icon = icons[status]
  return (
    <span className={`inline-flex items-center gap-1 text-xs font-medium ${toneText[tone]}`}>
      <Icon className={`size-3.5 ${status === 'connecting' ? 'animate-spin' : ''}`} aria-hidden />
      {label}
      {demo && <span className="text-muted">{t('(démo)', '(demo)')}</span>}
    </span>
  )
}
