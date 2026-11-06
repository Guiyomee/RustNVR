// Centralized HTTP client for /api/v1: timeouts, typed errors, JSON decoding.
// Every API call goes through `apiGet`; components never call fetch directly.

import type { ApiErrorBody } from '../types/api'
import { t } from '../lib/i18n'

export const API_BASE = '/api/v1'
const DEFAULT_TIMEOUT_MS = 10_000

/** Dispatched on `window` when the server answers 401 (session over). */
export const UNAUTHORIZED_EVENT = 'rustnvr:unauthorized'

export type ApiErrorKind = 'http' | 'auth' | 'timeout' | 'network' | 'unavailable' | 'decode'

/** Every failure of an API call. `kind` drives how the interface reports it. */
export class ApiError extends Error {
  readonly kind: ApiErrorKind
  readonly status: number | null
  readonly code: string | null

  constructor(kind: ApiErrorKind, message: string, status: number | null = null, code: string | null = null) {
    super(message)
    this.name = 'ApiError'
    this.kind = kind
    this.status = status
    this.code = code
  }
}

/** Thrown for features the backend does not implement yet (see API_CONTRACT.md). */
export class UnavailableError extends ApiError {
  readonly route: string
  constructor(route: string, message: string) {
    super('unavailable', message)
    this.route = route
  }
}

export function apiGet<T>(path: string, timeoutMs = DEFAULT_TIMEOUT_MS, signal?: AbortSignal): Promise<T> {
  return apiRequest<T>('GET', path, undefined, timeoutMs, signal)
}

/** POST/PUT/DELETE with a JSON body. Resolves to `null` for 204 responses. */
export function apiSend<T>(method: 'POST' | 'PUT' | 'DELETE', path: string, body?: unknown, timeoutMs = DEFAULT_TIMEOUT_MS): Promise<T> {
  return apiRequest<T>(method, path, body, timeoutMs)
}

/** POST with a JSON body, answered by a file (errors are JSON as usual). */
export function apiSendForFile(path: string, body: unknown, timeoutMs = DEFAULT_TIMEOUT_MS): Promise<Blob> {
  return apiRequest<Blob>('POST', path, body, timeoutMs, undefined, true)
}

async function apiRequest<T>(method: string, path: string, body: unknown, timeoutMs: number, signal?: AbortSignal, file = false): Promise<T> {
  const controller = new AbortController()
  const timer = window.setTimeout(() => controller.abort('timeout'), timeoutMs)
  const onAbort = () => controller.abort('cancelled')
  signal?.addEventListener('abort', onAbort, { once: true })
  let response: Response
  try {
    response = await fetch(API_BASE + path, {
      method,
      signal: controller.signal,
      headers: body === undefined ? { Accept: 'application/json' } : { Accept: 'application/json', 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
      credentials: 'same-origin',
    })
  } catch (e) {
    if (controller.signal.aborted && controller.signal.reason === 'timeout') {
      throw new ApiError('timeout', t(`Le serveur n'a pas répondu en ${timeoutMs / 1000} s`, `The server did not answer within ${timeoutMs / 1000} s`))
    }
    if (signal?.aborted) throw e
    throw new ApiError('network', t('Serveur RustNVR injoignable', 'RustNVR server unreachable'))
  } finally {
    window.clearTimeout(timer)
    signal?.removeEventListener('abort', onAbort)
  }

  if (!response.ok) {
    // A 401 outside the login routes means the session ended: the login
    // page takes over. Login routes report their own message below.
    if (response.status === 401 && !path.startsWith('/auth/')) {
      window.dispatchEvent(new Event(UNAUTHORIZED_EVENT))
      throw new ApiError('auth', t('Authentification requise', 'Sign-in required'), 401)
    }
    let code: string | null = null
    let message = t(`Erreur HTTP ${response.status}`, `HTTP error ${response.status}`)
    try {
      const body = (await response.json()) as ApiErrorBody
      code = body.error.code
      message = body.error.message
    } catch {
      // Not a JSON error body (proxy, old server…): keep the generic message.
    }
    throw new ApiError('http', message, response.status, code)
  }
  if (response.status === 204) return null as T
  if (file) return (await response.blob()) as T
  try {
    return (await response.json()) as T
  } catch {
    throw new ApiError('decode', t('Réponse du serveur illisible', 'Unreadable server answer'), response.status)
  }
}

/** Human-readable description of any error, for ErrorState. */
export function describeError(e: unknown): string {
  if (e instanceof ApiError) return e.message
  if (e instanceof Error) return e.message
  return String(e)
}
