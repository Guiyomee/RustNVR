import { useEffect, useRef, useState, type ReactNode } from 'react'

/** Button-anchored panel closing on outside click and Escape. */
export function Popover({ trigger, children, align = 'right', onOpen }: { trigger: (open: boolean, toggle: () => void) => ReactNode; children: ReactNode; align?: 'left' | 'right'; onOpen?: () => void }) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && setOpen(false)
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])
  const toggle = () => {
    if (!open) onOpen?.()
    setOpen(!open)
  }
  return (
    <div ref={ref} className="relative">
      {trigger(open, toggle)}
      {open && (
        <div className={`card absolute top-full z-40 mt-2 w-80 max-w-[calc(100vw-1.5rem)] shadow-xl shadow-black/40 ${align === 'right' ? 'right-0' : 'left-0'}`}>
          {children}
        </div>
      )}
    </div>
  )
}
