import { CirclePlay, HardDrive, Info, LayoutDashboard, MonitorPlay, Settings, Activity, Video } from 'lucide-react'
import { t } from '../../lib/i18n'

export const NAV_ITEMS = [
  { to: '/', label: t("Vue d'ensemble", "Overview"), icon: LayoutDashboard },
  { to: '/cameras', label: t('Caméras', 'Cameras'), icon: Video },
  { to: '/events', label: t('Événements', 'Events'), icon: Activity },
  { to: '/playback', label: t('Lecture', 'Playback'), icon: CirclePlay },
  { to: '/mur', label: t("Mur d'écrans", "Video wall"), icon: MonitorPlay },
  { to: '/storage', label: t('Stockage', 'Storage'), icon: HardDrive },
  { to: '/settings', label: t('Paramètres', 'Settings'), icon: Settings },
  { to: '/about', label: t('À propos', 'About'), icon: Info },
] as const

export function isActive(itemTo: string, path: string): boolean {
  return itemTo === '/' ? path === '/' : path === itemTo || path.startsWith(`${itemTo}/`)
}
