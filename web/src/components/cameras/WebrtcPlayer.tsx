import { LoaderCircle, TriangleAlert } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { API_BASE, UNAUTHORIZED_EVENT } from '../../api/client'
import type { LiveStream } from '../../api/endpoints'
import { t } from '../../lib/i18n'

type Status = { kind: 'connecting' } | { kind: 'playing' } | { kind: 'error'; message: string }

/**
 * Low-latency live view over WebRTC (WHEP): the browser sends its offer, the
 * server answers and sends H.264 over UDP (H.265 cameras go through the
 * H.264 conversion). Needs a direct UDP path to the server (local network).
 */
export function WebrtcPlayer({ cameraId, stream, large = false }: { cameraId: string; stream?: LiveStream; large?: boolean }) {
  const videoRef = useRef<HTMLVideoElement>(null)
  const [status, setStatus] = useState<Status>({ kind: 'connecting' })

  useEffect(() => {
    const video = videoRef.current
    if (!video) return
    if (typeof RTCPeerConnection === 'undefined') {
      setStatus({ kind: 'error', message: t('Ce navigateur ne prend pas en charge WebRTC.', 'This browser does not support WebRTC.') })
      return
    }
    let closed = false
    const pc = new RTCPeerConnection()
    pc.addTransceiver('video', { direction: 'recvonly' })
    pc.ontrack = (e) => {
      video.srcObject = e.streams[0] ?? new MediaStream([e.track])
      void video.play().catch(() => undefined)
    }
    pc.oniceconnectionstatechange = () => {
      if (closed) return
      if (pc.iceConnectionState === 'failed') {
        setStatus({ kind: 'error', message: t('Connexion WebRTC impossible : aucun chemin UDP direct vers le serveur (réseau, pare-feu). Utilisez le direct standard.', 'WebRTC connection failed: no direct UDP path to the server (network, firewall). Use the standard live view.') })
      }
    }
    // Without any connection after a while, explain instead of spinning.
    const timer = window.setTimeout(() => {
      if (!closed && pc.iceConnectionState !== 'connected' && pc.iceConnectionState !== 'completed') {
        setStatus({ kind: 'error', message: t('Connexion WebRTC trop longue : le serveur est peut-être injoignable en UDP. Utilisez le direct standard.', 'WebRTC connection too slow: the server may not be reachable over UDP. Use the standard live view.') })
      }
    }, 15_000)
    void (async () => {
      try {
        await pc.setLocalDescription(await pc.createOffer())
        // Non-trickle WHEP: wait for the candidates (bounded).
        await new Promise<void>((resolve) => {
          if (pc.iceGatheringState === 'complete') return resolve()
          pc.onicegatheringstatechange = () => pc.iceGatheringState === 'complete' && resolve()
          window.setTimeout(resolve, 2_000)
        })
        const q = stream ? `?stream=${stream}` : ''
        const res = await fetch(`${API_BASE}/cameras/${encodeURIComponent(cameraId)}/live/webrtc${q}`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/sdp' },
          body: pc.localDescription?.sdp ?? '',
          credentials: 'same-origin',
        })
        const body = await res.text()
        if (res.status === 401) window.dispatchEvent(new Event(UNAUTHORIZED_EVENT))
        if (res.status !== 201) {
          let message = t(`Erreur ${res.status}`, `Error ${res.status}`)
          try {
            message = (JSON.parse(body) as { error: { message: string } }).error.message
          } catch {
            // keep the generic message
          }
          throw new Error(message)
        }
        if (closed) return
        await pc.setRemoteDescription({ type: 'answer', sdp: body })
      } catch (e) {
        if (!closed) setStatus({ kind: 'error', message: e instanceof Error ? e.message : String(e) })
      }
    })()
    return () => {
      closed = true
      window.clearTimeout(timer)
      pc.close()
    }
  }, [cameraId, stream])

  return (
    <div className="relative h-full w-full bg-black">
      <video
        ref={videoRef}
        muted
        autoPlay
        playsInline
        className="h-full w-full object-contain"
        onPlaying={() => setStatus({ kind: 'playing' })}
      />
      <span className="pointer-events-none absolute top-2 right-2 rounded bg-black/60 px-1.5 py-0.5 text-[10px] font-semibold text-ok">WEBRTC</span>
      {status.kind === 'connecting' && (
        <div className="absolute inset-0 flex items-center justify-center text-muted">
          <LoaderCircle className={`${large ? 'size-8' : 'size-5'} animate-spin`} aria-label={t('Connexion WebRTC', 'WebRTC connection')} />
        </div>
      )}
      {status.kind === 'error' && (
        <div className="absolute inset-0 flex flex-col items-center justify-center gap-2 p-4 text-center text-sm text-muted">
          <TriangleAlert className="size-5 text-warn" aria-hidden />
          {status.message}
        </div>
      )}
    </div>
  )
}
