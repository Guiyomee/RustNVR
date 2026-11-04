import { recordingState } from '../../lib/recording'
import type { Camera } from '../../types/api'

const TONES = {
  rec: 'bg-err/15 text-err',
  idle: 'bg-accent/15 text-accent',
  warn: 'bg-warn/15 text-warn',
  off: 'bg-line/70 text-muted',
}

/** Recording state of a camera right now (« REC », « Sur mouvement »…). */
export function RecordingBadge({ camera, overlay = false }: { camera: Camera; overlay?: boolean }) {
  const s = recordingState(camera)
  return (
    <span
      title={s.title}
      className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-semibold uppercase tracking-wide ${overlay ? 'bg-black/60 ' + TONES[s.tone].split(' ')[1] : TONES[s.tone]}`}
    >
      <span className={`size-1.5 rounded-full bg-current ${s.tone === 'rec' ? 'animate-pulse' : ''}`} aria-hidden />
      {s.label}
    </span>
  )
}
