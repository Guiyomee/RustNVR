/**
 * The RustNVR mark (« objectif cuivre »): a camera lens with a copper ring on
 * a slate tile. Same drawing as public/favicon.svg; its colors are fixed (the
 * mark reads the same on both themes), the wordmark follows the theme.
 */
export function Logo({ compact = false }: { compact?: boolean }) {
  return (
    <div className="flex items-center gap-2.5">
      <svg viewBox="0 0 32 32" className="size-8 shrink-0" aria-hidden>
        <rect width="32" height="32" rx="8" fill="#1C2128" />
        <circle cx="16" cy="16" r="10" fill="none" stroke="#C8642B" strokeWidth="3" />
        <circle cx="16" cy="16" r="5.5" fill="#C8642B" />
        <circle cx="16" cy="16" r="2.3" fill="#1C2128" />
        <circle cx="13.3" cy="13.2" r="1.3" fill="#F4F1EC" />
      </svg>
      {!compact && (
        <span className="text-base font-semibold tracking-tight">
          Rust<span className="text-brand">NVR</span>
        </span>
      )}
    </div>
  )
}
