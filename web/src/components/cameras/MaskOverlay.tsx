/**
 * Privacy masks over a video shown with `object-contain`: the SVG has the
 * picture's aspect ratio and the same « meet » fit, so the black rectangles
 * fall on the picture whatever the letterboxing.
 */
export function MaskOverlay({ masks, width, height }: { masks: number[][]; width: number | null; height: number | null }) {
  if (masks.length === 0) return null
  const w = width && width > 0 ? width : 1600
  const h = height && height > 0 ? height : 900
  return (
    <svg className="pointer-events-none absolute inset-0 h-full w-full" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="xMidYMid meet" aria-hidden>
      {masks.map(([x1 = 0, y1 = 0, x2 = 0, y2 = 0], i) => (
        <rect key={i} x={x1 * w} y={y1 * h} width={(x2 - x1) * w} height={(y2 - y1) * h} fill="black" />
      ))}
    </svg>
  )
}
