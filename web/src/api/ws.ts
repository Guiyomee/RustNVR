// WebSocket to /api/v1/ws with automatic reconnection (exponential backoff + jitter).

import type { WsMessage } from '../types/api'

export type SocketStatus = 'connecting' | 'open' | 'reconnecting' | 'closed'

export interface LiveSocketHandlers {
  onMessage: (msg: WsMessage) => void
  onStatus: (status: SocketStatus) => void
}

const INITIAL_DELAY_MS = 1_000
const MAX_DELAY_MS = 30_000

export class LiveSocket {
  private socket: WebSocket | null = null
  private delay = INITIAL_DELAY_MS
  private timer: number | null = null
  private stopped = false
  private readonly url: string
  private readonly handlers: LiveSocketHandlers

  constructor(url: string, handlers: LiveSocketHandlers) {
    this.url = url
    this.handlers = handlers
  }

  start(): void {
    this.stopped = false
    this.connect('connecting')
  }

  stop(): void {
    this.stopped = true
    if (this.timer !== null) window.clearTimeout(this.timer)
    this.timer = null
    this.socket?.close()
    this.socket = null
    this.handlers.onStatus('closed')
  }

  private connect(status: SocketStatus): void {
    this.handlers.onStatus(status)
    const socket = new WebSocket(this.url)
    this.socket = socket
    socket.onopen = () => {
      this.delay = INITIAL_DELAY_MS
      this.handlers.onStatus('open')
    }
    socket.onmessage = (ev: MessageEvent) => {
      if (typeof ev.data !== 'string') return
      try {
        this.handlers.onMessage(JSON.parse(ev.data) as WsMessage)
      } catch {
        // Ignore malformed messages rather than breaking the connection.
      }
    }
    socket.onclose = () => {
      if (this.socket !== socket || this.stopped) return
      this.socket = null
      this.scheduleReconnect()
    }
    // onclose always follows onerror; reconnection is handled there.
    socket.onerror = () => {}
  }

  private scheduleReconnect(): void {
    this.handlers.onStatus('reconnecting')
    const jitter = 1 - Math.random() * 0.2
    const wait = this.delay * jitter
    this.delay = Math.min(this.delay * 2, MAX_DELAY_MS)
    this.timer = window.setTimeout(() => {
      this.timer = null
      if (!this.stopped) this.connect('reconnecting')
    }, wait)
  }
}
