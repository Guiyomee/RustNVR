// Login gate: checks the session before showing the application, shows the
// login page (or the creation of the first administrator), and comes back to
// it when the server reports that the session ended.

import { LogIn, ShieldCheck } from 'lucide-react'
import { createContext, useCallback, useContext, useEffect, useState, type FormEvent, type ReactNode } from 'react'
import { UNAUTHORIZED_EVENT, describeError } from '../api/client'
import { getAuthStatus, login, logout, setupAdmin, type AuthStatus, type AuthUser } from '../api/endpoints'
import { Logo } from '../components/layout/Logo'
import { Button } from '../components/ui/Button'
import { LoadingState } from '../components/ui/States'
import { t } from '../lib/i18n'

interface AuthContextValue {
  status: AuthStatus | null
  user: AuthUser | null
  /** Re-reads the session state (after an account change). */
  refresh: () => void
  /** Opens the creation of the first administrator (no account yet). */
  startSetup: () => void
  signOut: () => Promise<void>
}

const AuthContext = createContext<AuthContextValue>({ status: null, user: null, refresh: () => undefined, startSetup: () => undefined, signOut: async () => undefined })

export function useAuth(): AuthContextValue {
  return useContext(AuthContext)
}

/** Why the signed-in account cannot change anything (viewer role), or null. */
export function useReadOnly(): string | null {
  const { user } = useAuth()
  return user?.role === 'viewer' ? t('Compte en lecture seule (rôle lecteur) : seuls les administrateurs peuvent modifier.', 'Read-only account (viewer role): only administrators can make changes.') : null
}

/**
 * True for a viewer account: it has no page to manage the cameras, the
 * storage or the settings (the server refuses them too).
 */
export function useIsViewer(): boolean {
  return useAuth().user?.role === 'viewer'
}

const inputCls = 'mt-1 h-10 w-full rounded-md border border-line bg-bg-2 px-3 text-sm text-fg outline-none focus:border-accent'

function AuthForm({ setup, onDone }: { setup: boolean; onDone: () => void }) {
  const [username, setUsername] = useState(setup ? 'admin' : '')
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    if (setup && password !== confirm) {
      setError(t('Les deux mots de passe diffèrent.', 'The two passwords differ.'))
      return
    }
    setBusy(true)
    setError(null)
    try {
      await (setup ? setupAdmin(username.trim(), password) : login(username.trim(), password))
      onDone()
    } catch (err) {
      setError(describeError(err))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="flex min-h-screen items-center justify-center bg-bg p-4">
      <form onSubmit={(e) => void submit(e)} className="card w-full max-w-sm space-y-4 p-6">
        <div className="flex items-center gap-2">
          <Logo />
        </div>
        <div>
          <h1 className="text-lg font-semibold">{setup ? t('Créer le compte administrateur', 'Create the administrator account') : t('Connexion', 'Sign in')}</h1>
          {setup && <p className="mt-1 text-sm text-muted">{t("Premier démarrage : ce compte pourra ensuite créer d'autres comptes (Paramètres → Utilisateurs).", "First start: this account will then be able to create other accounts (Settings → Users).")}</p>}
        </div>
        <label className="block text-xs font-medium text-muted">
          {t('Identifiant', 'Username')}
          <input className={inputCls} value={username} autoComplete="username" autoFocus={!setup} onChange={(e) => setUsername(e.target.value)} />
        </label>
        <label className="block text-xs font-medium text-muted">
          {t('Mot de passe', 'Password')}
          <input className={inputCls} type="password" value={password} autoComplete={setup ? 'new-password' : 'current-password'} autoFocus={setup} onChange={(e) => setPassword(e.target.value)} />
        </label>
        {setup && (
          <label className="block text-xs font-medium text-muted">
            {t('Confirmation', 'Confirmation')}
            <input className={inputCls} type="password" value={confirm} autoComplete="new-password" onChange={(e) => setConfirm(e.target.value)} />
            <span className="mt-1 block font-normal">{t('8 caractères au moins.', 'At least 8 characters.')}</span>
          </label>
        )}
        {error && <p className="rounded-md border border-err/40 bg-err/10 px-3 py-2 text-sm text-err">{error}</p>}
        <Button
          variant="primary"
          type="submit"
          className="w-full justify-center"
          icon={setup ? <ShieldCheck className="size-4" /> : <LogIn className="size-4" />}
          disabledReason={!username.trim() || !password ? t('Identifiant et mot de passe requis.', 'Username and password required.') : busy ? t('Vérification…', 'Checking…') : null}
        >
          {setup ? t('Créer et se connecter', 'Create and sign in') : t('Se connecter', 'Sign in')}
        </Button>
      </form>
    </div>
  )
}

export function AuthGate({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<AuthStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [setupOpen, setSetupOpen] = useState(false)
  const refresh = useCallback(() => {
    getAuthStatus().then(
      (s) => {
        setStatus(s)
        setError(null)
      },
      (e: unknown) => setError(describeError(e)),
    )
  }, [])
  useEffect(() => {
    refresh()
    window.addEventListener(UNAUTHORIZED_EVENT, refresh)
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, refresh)
  }, [refresh])
  const signOut = useCallback(async () => {
    await logout().catch(() => undefined)
    window.location.assign(import.meta.env.BASE_URL)
  }, [])

  // Servers without accounts answer the status route only with Basic
  // credentials (the browser asks for them): show the application anyway.
  if (error && !status) return <>{children}</>
  if (!status) return <LoadingState />
  if (status.required && !status.user) return <AuthForm setup={false} onDone={refresh} />
  if (setupOpen) return <AuthForm setup onDone={() => { setSetupOpen(false); refresh() }} />
  return (
    <AuthContext.Provider value={{ status, user: status.user, refresh, startSetup: () => setSetupOpen(true), signOut }}>
      {children}
    </AuthContext.Provider>
  )
}
