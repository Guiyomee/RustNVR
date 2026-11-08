import { X } from 'lucide-react'
import { useEffect, useRef, type ReactNode } from 'react'
import { Button, IconButton } from './Button'
import { t } from '../../lib/i18n'

interface ModalProps {
  open: boolean
  title: string
  onClose: () => void
  children: ReactNode
  footer?: ReactNode
  wide?: boolean
  /** Forms: a click on the backdrop does nothing and Escape only calls
   * `onClose`, which may ask before discarding what was typed. */
  guarded?: boolean
}

/** Native <dialog>: focus trapping, Escape and backdrop come from the browser. */
export function Modal({ open, title, onClose, children, footer, wide = false, guarded = false }: ModalProps) {
  const ref = useRef<HTMLDialogElement>(null)
  // The browser fires `close` after the page closed the dialog itself: only a
  // close by the user (Escape) is reported, or it would undo what the page
  // did meanwhile (another dialog opened in its place).
  const openRef = useRef(open)
  openRef.current = open
  // A click is on the backdrop only when the button also went down there: a
  // text selection started in a field and released outside is not one.
  const downOnBackdrop = useRef(false)
  useEffect(() => {
    const d = ref.current
    if (!d) return
    if (open && !d.open) d.showModal()
    if (!open && d.open) d.close()
  }, [open])
  return (
    <dialog
      ref={ref}
      onCancel={(e) => {
        if (!guarded) return
        e.preventDefault()
        onClose()
      }}
      onClose={() => {
        if (!openRef.current) return
        // Closed by the browser anyway (a second Escape): the page decides.
        if (guarded) ref.current?.showModal()
        onClose()
      }}
      onMouseDown={(e) => {
        downOnBackdrop.current = e.target === ref.current
      }}
      onClick={(e) => {
        const backdrop = downOnBackdrop.current && e.target === ref.current
        downOnBackdrop.current = false
        if (backdrop && !guarded) onClose()
      }}
      className={`card m-auto w-[calc(100%-2rem)] p-0 text-fg backdrop:bg-black/60 ${wide ? 'max-w-3xl' : 'max-w-lg'}`}
    >
      {open && (
        <div className="flex max-h-[85vh] flex-col">
          <header className="flex items-center justify-between border-b border-line px-5 py-3">
            <h2 className="font-semibold">{title}</h2>
            <IconButton label={t('Fermer', 'Close')} icon={<X className="size-4" />} onClick={onClose} />
          </header>
          <div className="overflow-y-auto px-5 py-4">{children}</div>
          {footer && <footer className="flex justify-end gap-2 border-t border-line px-5 py-3">{footer}</footer>}
        </div>
      )}
    </dialog>
  )
}

export function ConfirmDialog({
  open,
  title,
  message,
  confirmLabel = t('Confirmer', 'Confirm'),
  danger = false,
  onConfirm,
  onCancel,
}: {
  open: boolean
  title: string
  message: ReactNode
  confirmLabel?: string
  danger?: boolean
  onConfirm: () => void
  onCancel: () => void
}) {
  return (
    <Modal
      open={open}
      title={title}
      onClose={onCancel}
      footer={
        <>
          <Button onClick={onCancel}>{t('Annuler', 'Cancel')}</Button>
          <Button variant={danger ? 'danger' : 'primary'} onClick={onConfirm}>
            {confirmLabel}
          </Button>
        </>
      }
    >
      <div className="text-sm text-muted">{message}</div>
    </Modal>
  )
}
