import { CirclePlay, HardDrive, Images, Info, LayoutDashboard, MonitorPlay, Settings, Activity, Video } from 'lucide-react'
import { t } from '../../lib/i18n'

export const NAV_ITEMS = [
  { to: '/', label: t("Vue d'ensemble", "Overview"), icon: LayoutDashboard },
  { to: '/cameras', label: t('Caméras', 'Cameras'), icon: Video },
  { to: '/events', label: t('Événements', 'Events'), icon: Activity },
  { to: '/images', label: t('Images', 'Pictures'), icon: Images },
  { to: '/playback', label: t('Lecture', 'Playback'), icon: CirclePlay },
  { to: '/mur', label: t("Mur d'écrans", "Video wall"), icon: MonitorPlay },
  { to: '/storage', label: t('Stockage', 'Storage'), icon: HardDrive },
  { to: '/settings', label: t('Paramètres', 'Settings'), icon: Settings },
  { to: '/about', label: t('À propos', 'About'), icon: Info },
] as const

/** Pages of the administrators: a viewer cannot change anything there. */
const ADMIN_PAGES: readonly string[] = ['/cameras', '/storage']

/** True for a page a viewer account has no use of (the detail of a camera stays). */
export function adminPage(path: string): boolean {
  return ADMIN_PAGES.includes(path)
}

/** The menu of an account. */
export function navItems(viewer: boolean) {
  return viewer ? NAV_ITEMS.filter((item) => !adminPage(item.to)) : NAV_ITEMS
}

export function isActive(itemTo: string, path: string): boolean {
  return itemTo === '/' ? path === '/' : path === itemTo || path.startsWith(`${itemTo}/`)
}
