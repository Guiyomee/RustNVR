import { Activity, CirclePlay, Ellipsis, LayoutDashboard } from 'lucide-react'
import { t } from '../../lib/i18n'
import { Link, useLocation } from '../../lib/router'
import { isActive, NAV_ITEMS } from './nav'

/** The four entries of the phone bar; « Plus » opens the full menu. */
const MAIN = [
  { to: '/', label: t('Accueil', 'Home'), icon: LayoutDashboard },
  { to: '/events', label: t('Événements', 'Events'), icon: Activity },
  { to: '/playback', label: t('Lecture', 'Playback'), icon: CirclePlay },
] as const

const ENTRY = 'flex flex-col items-center gap-1 py-1.5 text-[11px] font-medium'

/** Navigation bar at the bottom of the screen, on phones and small tablets (below `lg`). */
export function MobileNav({ onMore }: { onMore: () => void }) {
  const { path } = useLocation()
  // « Plus » is lit on the pages it leads to (Cameras, Images, Settings…).
  const elsewhere = !MAIN.some((m) => isActive(m.to, path)) && NAV_ITEMS.some((n) => isActive(n.to, path))
  return (
    <nav aria-label={t('Navigation', 'Navigation')} className="fixed inset-x-0 bottom-0 z-40 grid grid-cols-4 border-t border-line bg-bg-2/95 px-2 pt-1.5 pb-[max(0.5rem,env(safe-area-inset-bottom))] backdrop-blur lg:hidden">
      {MAIN.map(({ to, label, icon: Icon }) => {
        const active = isActive(to, path)
        return (
          <Link key={to} to={to} aria-current={active ? 'page' : undefined} className={`${ENTRY} ${active ? 'text-accent' : 'text-muted'}`}>
            <Icon className="size-[22px]" strokeWidth={1.8} aria-hidden />
            {label}
          </Link>
        )
      })}
      <button type="button" onClick={onMore} className={`${ENTRY} ${elsewhere ? 'text-accent' : 'text-muted'}`}>
        <Ellipsis className="size-[22px]" strokeWidth={1.8} aria-hidden />
        {t('Plus', 'More')}
      </button>
    </nav>
  )
}
