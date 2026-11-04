import { PanelLeftClose, PanelLeftOpen, X } from 'lucide-react'
import { Link, useLocation } from '../../lib/router'
import { useIsDemo } from '../../data/DataContext'
import { Badge } from '../ui/Card'
import { IconButton } from '../ui/Button'
import { Logo } from './Logo'
import { NAV_ITEMS, isActive } from './nav'
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
  const nav = (compact: boolean, onNavigate?: () => void) => (
    <nav aria-label={t('Navigation principale', 'Main navigation')} className="flex flex-col gap-1 px-2">
      {NAV_ITEMS.map(({ to, label, icon: Icon }) => {
        const active = isActive(to, path)
        return (
          <Link
            key={to}
            to={to}
            onClick={onNavigate}
            aria-current={active ? 'page' : undefined}
            title={compact ? label : undefined}
            className={`flex h-10 items-center gap-3 rounded-md px-3 text-sm font-medium transition-colors ${
              active ? 'bg-accent text-white' : 'text-muted hover:bg-card hover:text-fg'
            } ${compact ? 'justify-center px-0' : ''}`}
          >
            <Icon className="size-[18px] shrink-0" aria-hidden />
            {!compact && <span className="truncate">{label}</span>}
          </Link>
        )
      })}
    </nav>
  )

  return (
    <>
      {/* Desktop */}
      <aside className={`sticky top-0 hidden h-screen shrink-0 flex-col border-r border-line bg-bg-2 lg:flex ${collapsed ? 'w-[68px]' : 'w-60'}`}>
        <div className={`flex h-16 items-center ${collapsed ? 'justify-center' : 'px-5'}`}>
          <Logo compact={collapsed} />
        </div>
        {demo && !collapsed && (
          <div className="px-5 pb-3">
            <Badge tone="demo">{t('Mode démo', 'Demo mode')}</Badge>
          </div>
        )}
        <div className="flex-1 overflow-y-auto pt-2">{nav(collapsed)}</div>
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
