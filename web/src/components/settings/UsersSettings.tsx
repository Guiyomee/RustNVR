import { KeyRound, Trash, UserPlus, Video } from 'lucide-react'
import { useState } from 'react'
import { describeError } from '../../api/client'
import { changePassword, createUser, deleteUser, getUsers, updateUser, type UserAccount } from '../../api/endpoints'
import { useAuth } from '../../auth/AuthGate'
import { useIsDemo, useLive } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { formatDate } from '../../lib/format'
import { Button, IconButton } from '../ui/Button'
import { Badge } from '../ui/Card'
import { ConfirmDialog } from '../ui/Modal'
import { ErrorState, LoadingState } from '../ui/States'
import { t } from '../../lib/i18n'

const inputCls = 'mt-1 h-9 w-full rounded-md border border-line bg-bg-2 px-3 text-sm text-fg outline-none focus:border-accent'
const MIN = 8

function OwnPassword() {
  const [current, setCurrent] = useState('')
  const [next, setNext] = useState('')
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null)
  const { signOut } = useAuth()
  const save = async () => {
    setBusy(true)
    setMessage(null)
    try {
      await changePassword(current, next)
      setMessage({ ok: true, text: t('Mot de passe changé : reconnectez-vous.', 'Password changed: sign in again.') })
      window.setTimeout(() => void signOut(), 1500)
    } catch (e) {
      setMessage({ ok: false, text: describeError(e) })
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="space-y-2">
      <h3 className="text-sm font-medium">{t('Mon mot de passe', 'My password')}</h3>
      <div className="grid max-w-lg gap-3 sm:grid-cols-2">
        <label className="text-xs font-medium text-muted">
          {t('Actuel', 'Current')}
          <input className={inputCls} type="password" autoComplete="current-password" value={current} onChange={(e) => setCurrent(e.target.value)} />
        </label>
        <label className="text-xs font-medium text-muted">
          {t('Nouveau (8 caractères au moins)', 'New (at least 8 characters)')}
          <input className={inputCls} type="password" autoComplete="new-password" value={next} onChange={(e) => setNext(e.target.value)} />
        </label>
      </div>
      <Button size="sm" icon={<KeyRound className="size-3.5" />} disabledReason={!current || next.length < MIN ? t('Mot de passe actuel et nouveau (8 caractères au moins).', 'Current and new password (at least 8 characters).') : busy ? t('Enregistrement…', 'Saving…') : null} onClick={() => void save()}>
        {t('Changer', 'Change')}
      </Button>
      {message && <p className={`text-xs ${message.ok ? 'text-ok' : 'text-err'}`}>{message.text}</p>}
    </div>
  )
}

/** The cameras a viewer sees: all of them, or the ticked ones. */
function CameraRights({ user, busy, onSave, onCancel }: { user: UserAccount; busy: boolean; onSave: (cameras: string[] | null) => void; onCancel: () => void }) {
  const cameras = useLive((s) => s.cameras)
  const [all, setAll] = useState(user.cameras === null)
  const [chosen, setChosen] = useState<string[]>(user.cameras ?? [])
  // A camera removed since is no longer offered, nor sent back.
  const kept = chosen.filter((id) => cameras.some((c) => c.id === id))
  const toggle = (id: string) => setChosen(kept.includes(id) ? kept.filter((c) => c !== id) : [...kept, id])
  return (
    <div className="space-y-3 rounded-md border border-line p-3">
      <p className="text-xs font-medium text-muted">{t('Caméras visibles par ', 'Cameras seen by ')}{user.username}</p>
      <div className="flex flex-wrap gap-4 text-sm">
        <label className="flex items-center gap-2">
          <input type="radio" name={`rights-${user.username}`} checked={all} onChange={() => setAll(true)} />
          {t('Toutes les caméras (celles ajoutées plus tard aussi)', 'Every camera (those added later too)')}
        </label>
        <label className="flex items-center gap-2">
          <input type="radio" name={`rights-${user.username}`} checked={!all} onChange={() => setAll(false)} />
          {t('Seulement celles cochées', 'Only the ticked ones')}
        </label>
      </div>
      {!all && (
        <div className="flex flex-wrap gap-x-4 gap-y-1.5 text-sm">
          {cameras.length === 0 && <span className="text-muted">{t('Aucune caméra configurée.', 'No camera configured.')}</span>}
          {cameras.map((c) => (
            <label key={c.id} className="flex items-center gap-2">
              <input type="checkbox" checked={kept.includes(c.id)} onChange={() => toggle(c.id)} />
              {c.name}
            </label>
          ))}
        </div>
      )}
      {!all && kept.length === 0 && <p className="text-xs text-warn">{t('Aucune caméra cochée : ce compte ne verra rien.', 'No camera ticked: this account will see nothing.')}</p>}
      <div className="flex gap-2">
        <Button size="sm" variant="primary" disabledReason={busy ? t('Enregistrement…', 'Saving…') : null} onClick={() => onSave(all ? null : kept)}>
          {t('Enregistrer', 'Save')}
        </Button>
        <Button size="sm" onClick={onCancel}>
          {t('Annuler', 'Cancel')}
        </Button>
      </div>
      <p className="text-xs text-muted">
        {t(
          "Les autres caméras n'existent pas pour ce compte : ni direct, ni enregistrements, ni événements, ni images. Appliqué tout de suite, sans reconnexion.",
          'The other cameras do not exist for this account: no live view, recordings, events or pictures. Applied at once, without signing in again.',
        )}
      </p>
    </div>
  )
}

function Accounts({ me }: { me: string | null }) {
  const list = useAsync(() => getUsers(), [])
  const [rights, setRights] = useState<string | null>(null)
  const [name, setName] = useState('')
  const [password, setPassword] = useState('')
  const [role, setRole] = useState<'admin' | 'viewer'>('viewer')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [toDelete, setToDelete] = useState<UserAccount | null>(null)
  const [reset, setReset] = useState<{ user: string; password: string } | null>(null)
  const run = async (action: () => Promise<unknown>) => {
    setBusy(true)
    setError(null)
    try {
      await action()
      list.reload()
      return true
    } catch (e) {
      setError(describeError(e))
      return false
    } finally {
      setBusy(false)
    }
  }
  if (list.error) return <ErrorState error={list.error} onRetry={list.reload} compact />
  if (!list.data) return <LoadingState compact />
  return (
    <div className="space-y-3">
      <h3 className="text-sm font-medium">{t('Comptes', 'Accounts')}</h3>
      <ul className="divide-y divide-line rounded-md border border-line">
        {list.data.map((u) => (
          <li key={u.username} className="flex flex-wrap items-center gap-3 px-3 py-2 text-sm">
            <span className="min-w-32 font-medium">{u.username}</span>
            {u.username === me && <Badge tone="info">{t('Vous', 'You')}</Badge>}
            <select
              className="h-8 rounded border border-line bg-bg-2 px-2 text-sm"
              value={u.role}
              disabled={busy}
              aria-label={t(`Rôle de ${u.username}`, `Role of ${u.username}`)}
              onChange={(e) => void run(() => updateUser(u.username, { role: e.target.value as 'admin' | 'viewer' }))}
            >
              <option value="admin">{t('Administrateur', 'Administrator')}</option>
              <option value="viewer">{t('Lecteur (consultation)', 'Viewer (read only)')}</option>
            </select>
            <span className="text-xs text-muted">{t('créé le ', 'created on ')}{formatDate(u.created_ms)}</span>
            {u.role === 'viewer' && (
              <Button size="sm" icon={<Video className="size-3.5" />} onClick={() => setRights(rights === u.username ? null : u.username)}>
                {u.cameras === null ? t('Toutes les caméras', 'Every camera') : t(`${u.cameras.length} caméra(s)`, `${u.cameras.length} camera(s)`)}
              </Button>
            )}
            <span className="ml-auto flex gap-1">
              <IconButton label={t('Nouveau mot de passe', 'New password')} icon={<KeyRound className="size-4" />} onClick={() => setReset({ user: u.username, password: '' })} />
              <IconButton label={t('Supprimer', 'Delete')} icon={<Trash className="size-4" />} disabledReason={u.username === me ? t('Vous ne pouvez pas supprimer votre propre compte.', 'You cannot delete your own account.') : null} onClick={() => setToDelete(u)} />
            </span>
          </li>
        ))}
      </ul>
      {list.data
        .filter((u) => u.username === rights && u.role === 'viewer')
        .map((u) => (
          <CameraRights
            key={u.username}
            user={u}
            busy={busy}
            onCancel={() => setRights(null)}
            onSave={(cameras) => void run(() => updateUser(u.username, { cameras })).then((ok) => ok && setRights(null))}
          />
        ))}
      {reset && (
        <div className="flex flex-wrap items-end gap-2 rounded-md border border-line p-3">
          <label className="text-xs font-medium text-muted">
            {t('Nouveau mot de passe de ', 'New password for ')}{reset.user}
            <input className={inputCls} type="password" autoComplete="new-password" value={reset.password} onChange={(e) => setReset({ ...reset, password: e.target.value })} />
          </label>
          <Button
            size="sm"
            variant="primary"
            disabledReason={reset.password.length < MIN ? t('8 caractères au moins.', 'At least 8 characters.') : busy ? t('Enregistrement…', 'Saving…') : null}
            onClick={() => void run(() => updateUser(reset.user, { password: reset.password })).then((ok) => ok && setReset(null))}
          >
            {t('Enregistrer', 'Save')}
          </Button>
          <Button size="sm" onClick={() => setReset(null)}>
            {t('Annuler', 'Cancel')}
          </Button>
        </div>
      )}
      <div className="grid gap-3 rounded-md border border-line p-3 sm:grid-cols-[1fr_1fr_auto_auto] sm:items-end">
        <label className="text-xs font-medium text-muted">
          {t('Identifiant', 'Username')}
          <input className={inputCls} value={name} autoComplete="off" onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="text-xs font-medium text-muted">
          {t('Mot de passe', 'Password')}
          <input className={inputCls} type="password" autoComplete="new-password" value={password} onChange={(e) => setPassword(e.target.value)} />
        </label>
        <select className="h-9 rounded-md border border-line bg-bg-2 px-2 text-sm" value={role} onChange={(e) => setRole(e.target.value as 'admin' | 'viewer')} aria-label={t('Rôle', 'Role')}>
          <option value="viewer">{t('Lecteur', 'Viewer')}</option>
          <option value="admin">{t('Administrateur', 'Administrator')}</option>
        </select>
        <Button
          size="sm"
          variant="primary"
          icon={<UserPlus className="size-3.5" />}
          disabledReason={!name.trim() || password.length < MIN ? t('Identifiant et mot de passe (8 caractères au moins).', 'Username and password (at least 8 characters).') : busy ? t('Création…', 'Creating…') : null}
          onClick={() =>
            void run(() => createUser(name.trim(), password, role)).then((ok) => {
              if (ok) {
                setName('')
                setPassword('')
              }
            })
          }
        >
          {t('Ajouter', 'Add')}
        </Button>
      </div>
      {error && <p className="text-sm text-err">{error}</p>}
      <p className="text-xs text-muted">{t("Lecteur : voit le direct, les enregistrements et les événements de ses caméras, sans rien pouvoir modifier ; il n'a pas les pages Caméras, Stockage ni les réglages.", 'Viewer: sees the live view, the recordings and the events of its cameras, without being able to change anything; it has no Cameras or Storage page, nor the settings.')}</p>
      <ConfirmDialog
        open={toDelete !== null}
        title={t('Supprimer ce compte ?', 'Delete this account?')}
        danger
        confirmLabel={t('Supprimer', 'Delete')}
        message={toDelete && <>{t('Le compte ', 'The account ')}<strong className="text-fg">{toDelete.username}</strong>{t(' ne pourra plus se connecter (ses sessions sont fermées).', ' will no longer be able to sign in (its sessions are closed).')}</>}
        onCancel={() => setToDelete(null)}
        onConfirm={() => {
          if (toDelete) void run(() => deleteUser(toDelete.username)).then(() => setToDelete(null))
        }}
      />
    </div>
  )
}

/** Paramètres → Utilisateurs. */
export function UsersSettings() {
  const demo = useIsDemo()
  const { status, user, startSetup } = useAuth()
  if (demo) return <p className="px-5 py-4 text-sm text-muted">{t('Non disponible en mode démo.', 'Not available in demo mode.')}</p>
  if (!status?.accounts) {
    return <p className="px-5 py-4 text-sm text-muted">{t("Les comptes nécessitent la base de données, qui n'a pas pu être ouverte : l'accès reste protégé par web.username de config.toml.", "Accounts need the database, which could not be opened: access stays protected by web.username of config.toml.")}</p>
  }
  if (status.setup) {
    return (
      <div className="space-y-2 px-5 py-4 text-sm">
        <p className="text-warn">{t("Aucun compte : l'interface est accessible sans mot de passe à toute personne qui peut joindre cette adresse.", "No account: the interface is reachable without a password by anyone who can reach this address.")}</p>
        <Button variant="primary" onClick={startSetup}>
          {t('Créer le compte administrateur', 'Create the administrator account')}
        </Button>
      </div>
    )
  }
  return (
    <div className="space-y-6 px-5 py-4">
      {user?.username && <OwnPassword />}
      {user?.role === 'admin' ? <Accounts me={user.username} /> : <p className="text-sm text-muted">{t('Seuls les administrateurs gèrent les comptes.', 'Only administrators manage accounts.')}</p>}
    </div>
  )
}
