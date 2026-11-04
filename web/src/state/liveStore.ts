// Live state pushed by the WebSocket (or polled when it is down).
// Immutable snapshots + useSyncExternalStore: components re-render only when
// the slice they select changes.

import { useSyncExternalStore } from 'react'
import type { Camera, NvrEvent, Recording, SystemInfo } from '../types/api'
import { t } from '../lib/i18n'

export type Connection = 'connecting' | 'open' | 'reconnecting' | 'polling' | 'closed'

export interface Notification {
  id: number
  at: number
  level: 'error' | 'warning' | 'info'
  camera: string | null
  message: string
}

export interface LiveState {
  system: SystemInfo | null
  cameras: Camera[]
  /** False until the first camera list arrived. */
  loaded: boolean
  connection: Connection
  /** Last error contacting the API, if any. */
  error: string | null
  authRequired: boolean
  notifications: Notification[]
  unread: number
  recentSegments: Recording[]
  /** Latest detection events (newest first), as pushed by the server. */
  recentEvents: NvrEvent[]
  /** Incremented on each event change: event lists refetch when it changes. */
  eventsVersion: number
}

const MAX_NOTIFICATIONS = 50
const MAX_SEGMENTS = 50
const MAX_EVENTS = 50

export class LiveStore {
  private state: LiveState = {
    system: null,
    cameras: [],
    loaded: false,
    connection: 'connecting',
    error: null,
    authRequired: false,
    notifications: [],
    unread: 0,
    recentSegments: [],
    recentEvents: [],
    eventsVersion: 0,
  }
  private readonly listeners = new Set<() => void>()
  private nextId = 1

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  getState = (): LiveState => this.state

  set(patch: Partial<LiveState>): void {
    this.state = { ...this.state, ...patch }
    this.listeners.forEach((l) => l())
  }

  setCameras(cameras: Camera[]): void {
    const previous = new Map(this.state.cameras.map((c) => [c.id, c]))
    this.set({ cameras, loaded: true })
    for (const c of cameras) this.notifyTransition(previous.get(c.id), c)
  }

  upsertCamera(camera: Camera): void {
    const list = this.state.cameras
    const i = list.findIndex((c) => c.id === camera.id)
    const before = i >= 0 ? list[i] : undefined
    const cameras = i >= 0 ? list.map((c, k) => (k === i ? camera : c)) : [...list, camera]
    this.set({ cameras, loaded: true })
    this.notifyTransition(before, camera)
  }

  removeCamera(id: string): void {
    this.set({ cameras: this.state.cameras.filter((c) => c.id !== id) })
  }

  addSegment(recording: Recording): void {
    this.set({ recentSegments: [recording, ...this.state.recentSegments].slice(0, MAX_SEGMENTS) })
  }

  /** Adds or updates (same id) an event. */
  upsertEvent(event: NvrEvent): void {
    const rest = this.state.recentEvents.filter((e) => e.id !== event.id)
    this.set({
      recentEvents: [event, ...rest].sort((a, b) => b.start_ms - a.start_ms).slice(0, MAX_EVENTS),
      eventsVersion: this.state.eventsVersion + 1,
    })
  }

  notify(level: Notification['level'], message: string, camera: string | null = null): void {
    const n: Notification = { id: this.nextId++, at: Date.now(), level, camera, message }
    this.set({
      notifications: [n, ...this.state.notifications].slice(0, MAX_NOTIFICATIONS),
      unread: this.state.unread + 1,
    })
  }

  markAllRead(): void {
    if (this.state.unread > 0) this.set({ unread: 0 })
  }

  clearNotifications(): void {
    this.set({ notifications: [], unread: 0 })
  }

  private notifyTransition(before: Camera | undefined, after: Camera): void {
    if (!before || before.status === after.status) return
    if (before.status === 'online' && after.status !== 'online' && after.status !== 'stopped') {
      this.notify('error', t(`${after.name} : flux perdu${after.last_error ? ` (${after.last_error})` : ''}`, `${after.name}: stream lost${after.last_error ? ` (${after.last_error})` : ''}`), after.id)
    } else if (before.status !== 'online' && after.status === 'online') {
      this.notify('info', t(`${after.name} : flux rétabli`, `${after.name}: stream back`), after.id)
    }
  }
}

export function useLiveState<T>(store: LiveStore, select: (s: LiveState) => T): T {
  return useSyncExternalStore(store.subscribe, () => select(store.getState()))
}
