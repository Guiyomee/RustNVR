import { PanelLeftClose, PanelLeftOpen, X } from 'lucide-react'
import { Link, useLocation } from '../../lib/router'
import { useIsDemo } from '../../data/DataContext'
import { Badge } from '../ui/Card'
import { IconButton } from '../ui/Button'
import { Logo } from './Logo'
import { useIsViewer } from '../../auth/AuthGate'
import { isActive, navItems } from './nav'
import { t } from '../../lib/i18n'

interface Props {
  collapsed: boolean
  onToggleCollapsed: () => void
  /** Mobile drawer. */
  mobileOpen: boolean
  onCloseMobile: () => void
}

export function Sidebar({ collapsed, onToggleCollapsed, mobileOpen, onCloseMobile }: Props) {
  const { path } = useLocation()
  const demo = useIsDemo()
  const all = navItems(useIsViewer())
  // Settings and About at the bottom of the bar.
  const bottom = new Set<string>(['/settings', '/about'])
  const nav = (compact: boolean, onNavigate?: () => void) => {
    const item = (i: (typeof all)[number]) => <NavLink key={i.to} item={i} path={path} compact={compact} onNavigate={onNavigate} />
    return (
      <nav aria-label={t('Navigation principale', 'Main navigation')} className="flex h-full flex-col gap-1.5 px-3">
        {all.filter((i) => !bottom.has(i.to)).map(item)}
        <div className="flex-1" />
        {all.filter((i) => bottom.has(i.to)).map(item)}
      </nav>
    )
  }

  return (
    <>
      {/* Desktop */}
      <aside className={`sticky top-0 hidden h-screen shrink-0 flex-col border-r border-line bg-bg-2 lg:flex ${collapsed ? 'w-[72px]' : 'w-60'}`}>
        <div className={`flex h-[72px] items-center ${collapsed ? 'justify-center' : 'px-5'}`}>
          <Logo compact={collapsed} />
        </div>
        {demo && !collapsed && (
          <div className="px-5 pb-3">
            <Badge tone="demo">{t('Mode démo', 'Demo mode')}</Badge>
          </div>
        )}
        <div className="flex-1 overflow-y-auto pt-2 pb-2">{nav(collapsed)}</div>
        <div className={`border-t border-line p-2 ${collapsed ? 'flex justify-center' : ''}`}>
          <IconButton
            label={collapsed ? t('Déplier la barre latérale', 'Expand the sidebar') : t('Réduire la barre latérale', 'Collapse the sidebar')}
            icon={collapsed ? <PanelLeftOpen className="size-4" /> : <PanelLeftClose className="size-4" />}
            onClick={onToggleCollapsed}
          />
        </div>
      </aside>

      {/* Mobile drawer */}
      {mobileOpen && (
        <div className="fixed inset-0 z-50 lg:hidden" role="dialog" aria-modal="true" aria-label={t('Menu', 'Menu')}>
          <button type="button" aria-label={t('Fermer le menu', 'Close the menu')} className="absolute inset-0 bg-black/60" onClick={onCloseMobile} />
          <aside className="relative flex h-full w-72 max-w-[85vw] flex-col border-r border-line bg-bg-2">
            <div className="flex h-16 items-center justify-between px-5">
              <Logo />
              <IconButton label={t('Fermer le menu', 'Close the menu')} icon={<X className="size-4" />} onClick={onCloseMobile} />
            </div>
            {demo && (
              <div className="px-5 pb-3">
                <Badge tone="demo">{t('Mode démo', 'Demo mode')}</Badge>
              </div>
            )}
            <div className="flex-1 overflow-y-auto pt-2">{nav(false, onCloseMobile)}</div>
          </aside>
        </div>
      )}
    </>
  )
}

/** One entry of the bar: an icon in a 48 px square (rail), or with its label. */
function NavLink({ item, path, compact, onNavigate }: { item: ReturnType<typeof navItems>[number]; path: string; compact: boolean; onNavigate?: () => void }) {
  const { to, label, icon: Icon } = item
  const active = isActive(to, path)
  return (
    <Link
      to={to}
      onClick={onNavigate}
      aria-current={active ? 'page' : undefined}
      aria-label={compact ? label : undefined}
      title={compact ? label : undefined}
      className={`flex items-center gap-3 rounded-xl text-sm font-medium transition-colors ${
        active ? 'bg-accent/15 text-accent' : 'text-muted hover:bg-card hover:text-fg'
      } ${compact ? 'mx-auto size-12 justify-center' : 'h-11 px-3'}`}
    >
      <Icon className="size-[22px] shrink-0" strokeWidth={1.8} aria-hidden />
      {!compact && <span className="truncate">{label}</span>}
    </Link>
  )
}
