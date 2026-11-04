import { Cloud, HardDrive, Network, Pencil, Plus, Save, Star, Trash } from 'lucide-react'
import { useState } from 'react'
import { describeError } from '../../api/client'
import { useReadOnly } from '../../auth/AuthGate'
import { createDestination, deleteDestination, getDestinations, getSettings, setSettings, updateDestination } from '../../api/endpoints'
import { useCapability, useIsDemo, useLive } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { formatBytes, formatPercent } from '../../lib/format'
import type { Destination, DestinationInput, RemoteInput } from '../../types/api'
import { slugify } from '../cameras/CameraForm'
import { Button, IconButton } from '../ui/Button'
import { Badge } from '../ui/Card'
import { Meter } from '../ui/Meter'
import { ConfirmDialog, Modal } from '../ui/Modal'
import { ErrorState, LoadingState } from '../ui/States'
import { locale, t } from '../../lib/i18n'

const inputCls = 'h-9 w-full rounded-md border border-line bg-bg-2 px-3 text-sm outline-none focus:border-accent disabled:opacity-60'

/** Empty field → no limit. */
function parseLimit(v: string): number | null | 'invalid' {
  if (!v.trim()) return null
  const n = Number(v)
  return Number.isInteger(n) && n > 0 ? n : 'invalid'
}

function RetentionEditor({ dest, onSaved, disabledReason }: { dest: Destination; onSaved: (list: Destination[]) => void; disabledReason: string | null }) {
  const [days, setDays] = useState(dest.retention_days?.toString() ?? '')
  const [gb, setGb] = useState(dest.max_size_gb?.toString() ?? '')
  const [free, setFree] = useState(dest.min_free_gb?.toString() ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const d = parseLimit(days)
  const g = parseLimit(gb)
  const m = parseLimit(free)
  const changed =
    (dest.retention_days?.toString() ?? '') !== days.trim() || (dest.max_size_gb?.toString() ?? '') !== gb.trim() || (dest.min_free_gb?.toString() ?? '') !== free.trim()
  const invalid = d === 'invalid' || g === 'invalid' || m === 'invalid'
  const save = async () => {
    if (d === 'invalid' || g === 'invalid' || m === 'invalid') return
    setBusy(true)
    setError(null)
    try {
      // Name, path and mount requirement are sent unchanged.
      const input: DestinationInput = { name: dest.name, path: dest.path, require_mount: dest.require_mount, retention_days: d, max_size_gb: g, min_free_gb: m }
      onSaved(await updateDestination(dest.id, input))
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="space-y-1">
      <div className="flex flex-wrap items-end gap-2">
        <label className="text-xs text-muted">
          {t('Conserver (jours)', 'Keep (days)')}
          <input className={`${inputCls.replace('w-full', 'w-32')} mt-1`} inputMode="numeric" placeholder={t('illimité', 'unlimited')} value={days} disabled={disabledReason !== null} onChange={(e) => setDays(e.target.value)} />
        </label>
        <label className="text-xs text-muted">
          {t('Taille max. (Go)', 'Max. size (GB)')}
          <input className={`${inputCls.replace('w-full', 'w-32')} mt-1`} inputMode="numeric" placeholder={t('illimitée', 'unlimited')} value={gb} disabled={disabledReason !== null} onChange={(e) => setGb(e.target.value)} />
        </label>
        <label className="text-xs text-muted" title={t("Si l'espace libre passe en dessous, les vidéos les plus anciennes sont supprimées (vérifié chaque minute).", "If free space drops below it, the oldest videos are deleted (checked every minute).")}>
          {t('Espace libre min. (Go)', 'Min. free space (GB)')}
          <input className={`${inputCls.replace('w-full', 'w-32')} mt-1`} inputMode="numeric" placeholder="aucun" value={free} disabled={disabledReason !== null} onChange={(e) => setFree(e.target.value)} />
        </label>
        <Button
          size="sm"
          icon={<Save className="size-3.5" />}
          onClick={() => void save()}
          disabledReason={disabledReason ?? (invalid ? t('Nombres entiers positifs, ou vide pour illimité.', 'Positive whole numbers, or empty for unlimited.') : !changed ? t('Aucun changement.', 'No change.') : busy ? t('Enregistrement…', 'Saving…') : null)}
        >
          {t('Appliquer', 'Apply')}
        </Button>
      </div>
      {invalid && <p className="text-xs text-err">{t('Entrez un nombre entier positif, ou laissez vide pour ne pas limiter.', 'Enter a positive whole number, or leave empty for no limit.')}</p>}
      {error && <p className="text-xs text-err">{error}</p>}
      <p className="text-xs text-muted">
        {d === null && g === null && m === null
          ? t('Aucune limite : les vidéos ne sont jamais supprimées automatiquement.', 'No limit: videos are never deleted automatically.')
          : t('Les segments les plus anciens sont supprimés automatiquement (vérification toutes les 10 minutes).', 'The oldest segments are deleted automatically (checked every 10 minutes).')}
      </p>
    </div>
  )
}

type Kind = 'nas' | 's3' | 'sftp'

interface FormState {
  kind: Kind
  name: string
  id: string
  path: string
  requireMount: boolean
  days: string
  gb: string
  // S3
  endpoint: string
  region: string
  bucket: string
  prefix: string
  accessKey: string
  secret: string
  // SFTP
  host: string
  port: string
  username: string
  auth: 'password' | 'key'
  password: string
  privateKey: string
  remoteDir: string
  hostKey: string
}

const emptyForm: FormState = {
  kind: 'nas',
  name: '',
  id: '',
  path: '',
  requireMount: true,
  days: '',
  gb: '',
  endpoint: '',
  region: 'us-east-1',
  bucket: '',
  prefix: 'rustnvr',
  accessKey: '',
  secret: '',
  host: '',
  port: '22',
  username: '',
  auth: 'password',
  password: '',
  privateKey: '',
  remoteDir: 'rustnvr',
  hostKey: '',
}

function toForm(dest: Destination): FormState {
  const base = { ...emptyForm, name: dest.name, id: dest.id, path: dest.path, requireMount: dest.require_mount, days: dest.retention_days?.toString() ?? '', gb: dest.max_size_gb?.toString() ?? '' }
  const r = dest.remote
  if (r?.type === 's3') return { ...base, kind: 's3', endpoint: r.endpoint ?? '', region: r.region, bucket: r.bucket, prefix: r.prefix, accessKey: r.access_key_id }
  if (r?.type === 'sftp') {
    return { ...base, kind: 'sftp', host: r.host, port: String(r.port), username: r.username, auth: r.private_key ? 'key' : 'password', privateKey: r.private_key ?? '', remoteDir: r.remote_dir, hostKey: r.host_key ?? '' }
  }
  return base
}

/** Remote settings to send; secrets left empty keep the stored ones. */
function toRemote(f: FormState): RemoteInput | null {
  if (f.kind === 's3') {
    return { type: 's3', endpoint: f.endpoint.trim() || null, region: f.region.trim() || 'us-east-1', bucket: f.bucket.trim(), prefix: f.prefix.trim(), access_key_id: f.accessKey.trim(), secret_access_key: f.secret || null }
  }
  if (f.kind === 'sftp') {
    return {
      type: 'sftp',
      host: f.host.trim(),
      port: Number(f.port),
      username: f.username.trim(),
      password: f.auth === 'password' ? f.password || null : null,
      private_key: f.auth === 'key' ? f.privateKey.trim() || null : null,
      remote_dir: f.remoteDir.trim(),
      host_key: f.hostKey.trim() || null,
    }
  }
  return null
}

function DestinationForm({ open, dest, onClose, onSaved }: { open: boolean; dest: Destination | null; onClose: () => void; onSaved: (list: Destination[]) => void }) {
  const editing = dest !== null
  const [f, setF] = useState<FormState>(emptyForm)
  const [idTouched, setIdTouched] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [initFor, setInitFor] = useState<string | null | undefined>(undefined)
  // Reset when the dialog opens for another destination.
  const key = open ? (dest?.id ?? null) : undefined
  if (key !== initFor) {
    setInitFor(key)
    setError(null)
    setIdTouched(editing)
    setF(
      dest ? toForm(dest) : emptyForm,
    )
  }
  const d = parseLimit(f.days)
  const g = parseLimit(f.gb)
  const problems = [
    !f.name.trim() && t('Le nom est obligatoire.', 'The name is required.'),
    !editing && !/^[a-z0-9][a-z0-9_-]{0,63}$/.test(f.id) && t("L'identifiant doit utiliser a-z, 0-9, - et _.", "The identifier must use a-z, 0-9, - and _."),
    !editing && f.id === 'local' && t("L'identifiant « local » est réservé.", "The « local » identifier is reserved."),
    (f.kind === 'nas' ? !f.path.startsWith('/') : f.path.trim() !== '' && !f.path.startsWith('/')) && t('Le chemin doit être absolu (commencer par /).', 'The path must be absolute (start with /).'),
    f.kind === 's3' && (!f.bucket.trim() || !f.accessKey.trim()) && t('Bucket et identifiant de clé obligatoires.', 'Bucket and key identifier are required.'),
    f.kind === 's3' && !(dest?.remote?.type === 's3' && dest.remote.has_secret) && !f.secret && t('La clé secrète est obligatoire.', 'The secret key is required.'),
    f.kind === 's3' && f.endpoint.trim() !== '' && !/^https?:\/\//.test(f.endpoint.trim()) && t("L'adresse du service doit commencer par https://.", "The service address must start with https://."),
    f.kind === 'sftp' && (!f.host.trim() || !f.username.trim()) && t('Serveur et utilisateur obligatoires.', 'Server and user are required.'),
    f.kind === 'sftp' && !(Number.isInteger(Number(f.port)) && Number(f.port) > 0 && Number(f.port) < 65536) && t('Port invalide.', 'Invalid port.'),
    f.kind === 'sftp' && f.auth === 'key' && !f.privateKey.startsWith('/') && t('Chemin absolu de la clé privée obligatoire.', 'Absolute path of the private key required.'),
    f.kind === 'sftp' && f.auth === 'password' && !(dest?.remote?.type === 'sftp' && dest.remote.has_password) && !f.password && t('Le mot de passe est obligatoire.', 'The password is required.'),
    f.kind === 'sftp' && f.hostKey.trim() !== '' && !f.hostKey.trim().startsWith('SHA256:') && t("L'empreinte doit commencer par SHA256:.", "The fingerprint must start with SHA256:."),
    (d === 'invalid' || g === 'invalid') && t('Rétention : nombres entiers positifs, ou vide.', 'Retention: positive whole numbers, or empty.'),
  ].filter(Boolean) as string[]

  const save = async () => {
    if (problems.length > 0 || d === 'invalid' || g === 'invalid') return
    setBusy(true)
    setError(null)
    try {
      // The reserve is edited next to the retention: kept unchanged here.
      const remote = toRemote(f)
      const input: DestinationInput = {
        name: f.name.trim(),
        path: f.path.trim(),
        require_mount: remote ? false : f.requireMount,
        retention_days: d,
        max_size_gb: g,
        min_free_gb: dest?.min_free_gb ?? null,
        remote,
      }
      onSaved(editing ? await updateDestination(dest.id, input) : await createDestination({ ...input, id: f.id }))
      onClose()
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal
      open={open}
      wide
      title={editing ? t(`Modifier ${dest.name}`, `Edit ${dest.name}`) : t('Ajouter un point de stockage', 'Add a storage destination')}
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>{t('Annuler', 'Cancel')}</Button>
          <Button variant="primary" disabledReason={problems[0] ?? (busy ? t('Vérification…', 'Checking…') : null)} onClick={() => void save()}>
            {busy ? t('Vérification…', 'Checking…') : t('Enregistrer', 'Save')}
          </Button>
        </>
      }
    >
      <div className="grid gap-4 sm:grid-cols-2">
        <label className="text-xs font-medium text-muted">
          {t('Nom', 'Name')}
          <input
            className={`${inputCls} mt-1`}
            value={f.name}
            placeholder="NAS Synology"
            onChange={(e) => setF((p) => ({ ...p, name: e.target.value, id: idTouched ? p.id : slugify(e.target.value) }))}
          />
        </label>
        <label className="text-xs font-medium text-muted">
          {t('Identifiant', 'Username')}
          <input
            className={`${inputCls} mt-1`}
            value={f.id}
            disabled={editing}
            onChange={(e) => {
              setIdTouched(true)
              setF((p) => ({ ...p, id: e.target.value }))
            }}
          />
        </label>
        <fieldset className="sm:col-span-2">
          <legend className="mb-1 text-xs font-medium text-muted">{t('Type', 'Type')}</legend>
          <div className="flex flex-wrap gap-4 text-sm">
            {(
              [
                ['nas', t('Dossier / partage NAS monté', 'Folder / mounted NAS share')],
                ['s3', t('Stockage S3 (cloud)', 'S3 storage (cloud)')],
                ['sftp', t('Serveur SFTP (SSH)', 'SFTP server (SSH)')],
              ] as const
            ).map(([k, label]) => (
              <label key={k} className="flex items-center gap-2">
                <input type="radio" name="dest-kind" className="accent-accent" checked={f.kind === k} disabled={editing} onChange={() => setF((p) => ({ ...p, kind: k }))} />
                {label}
              </label>
            ))}
          </div>
          {f.kind !== 'nas' && (
            <p className="mt-1 text-xs text-muted">
              {t("Les vidéos sont d'abord écrites dans un dossier d'attente local, puis envoyées segment par segment (toutes les minutes avec des segments de 60 s) et supprimées du dossier d'attente une fois envoyées. La relecture dans RustNVR n'est possible que pour les segments encore en attente.", "Videos are first written to a local outbox folder, then sent segment by segment (every minute with 60 s segments) and deleted from the outbox once sent. Playback in RustNVR is only possible for segments still waiting.")}
            </p>
          )}
        </fieldset>
        {f.kind === 's3' && (
          <>
            <label className="text-xs font-medium text-muted sm:col-span-2">
              {t('Adresse du service (vide pour Amazon S3)', 'Service address (empty for Amazon S3)')}
              <input className={`${inputCls} mt-1 font-mono`} value={f.endpoint} placeholder="https://s3.eu-central-003.backblazeb2.com" spellCheck={false} onChange={(e) => setF((p) => ({ ...p, endpoint: e.target.value }))} />
            </label>
            <label className="text-xs font-medium text-muted">
              {t('Bucket', 'Bucket')}
              <input className={`${inputCls} mt-1 font-mono`} value={f.bucket} spellCheck={false} onChange={(e) => setF((p) => ({ ...p, bucket: e.target.value }))} />
            </label>
            <label className="text-xs font-medium text-muted">
              {t('Région', 'Region')}
              <input className={`${inputCls} mt-1 font-mono`} value={f.region} spellCheck={false} onChange={(e) => setF((p) => ({ ...p, region: e.target.value }))} />
            </label>
            <label className="text-xs font-medium text-muted">
              {t('Identifiant de clé (access key)', 'Key identifier (access key)')}
              <input className={`${inputCls} mt-1 font-mono`} value={f.accessKey} autoComplete="off" spellCheck={false} onChange={(e) => setF((p) => ({ ...p, accessKey: e.target.value }))} />
            </label>
            <label className="text-xs font-medium text-muted">
              {t('Clé secrète', 'Secret key')}
              <input
                className={`${inputCls} mt-1`}
                type="password"
                autoComplete="new-password"
                value={f.secret}
                placeholder={dest?.remote?.type === 's3' && dest.remote.has_secret ? t('Inchangée (laisser vide)', 'Unchanged (leave empty)') : ''}
                onChange={(e) => setF((p) => ({ ...p, secret: e.target.value }))}
              />
            </label>
            <label className="text-xs font-medium text-muted sm:col-span-2">
              {t('Préfixe des fichiers', 'File prefix')}
              <input className={`${inputCls} mt-1 font-mono`} value={f.prefix} spellCheck={false} onChange={(e) => setF((p) => ({ ...p, prefix: e.target.value }))} />
            </label>
          </>
        )}
        {f.kind === 'sftp' && (
          <>
            <label className="text-xs font-medium text-muted">
              {t('Serveur', 'Server')}
              <input className={`${inputCls} mt-1 font-mono`} value={f.host} placeholder="192.168.1.20" spellCheck={false} onChange={(e) => setF((p) => ({ ...p, host: e.target.value }))} />
            </label>
            <label className="text-xs font-medium text-muted">
              {t('Port', 'Port')}
              <input className={`${inputCls} mt-1`} inputMode="numeric" value={f.port} onChange={(e) => setF((p) => ({ ...p, port: e.target.value }))} />
            </label>
            <label className="text-xs font-medium text-muted">
              {t('Utilisateur', 'User')}
              <input className={`${inputCls} mt-1`} value={f.username} autoComplete="off" onChange={(e) => setF((p) => ({ ...p, username: e.target.value }))} />
            </label>
            <label className="text-xs font-medium text-muted">
              {t('Dossier distant', 'Remote folder')}
              <input className={`${inputCls} mt-1 font-mono`} value={f.remoteDir} spellCheck={false} onChange={(e) => setF((p) => ({ ...p, remoteDir: e.target.value }))} />
            </label>
            <div className="flex gap-4 text-sm sm:col-span-2">
              <label className="flex items-center gap-2">
                <input type="radio" name="sftp-auth" className="accent-accent" checked={f.auth === 'password'} onChange={() => setF((p) => ({ ...p, auth: 'password' }))} />
                {t('Mot de passe', 'Password')}
              </label>
              <label className="flex items-center gap-2">
                <input type="radio" name="sftp-auth" className="accent-accent" checked={f.auth === 'key'} onChange={() => setF((p) => ({ ...p, auth: 'key' }))} />
                {t('Clé privée', 'Private key')}
              </label>
            </div>
            {f.auth === 'password' ? (
              <label className="text-xs font-medium text-muted sm:col-span-2">
                {t('Mot de passe', 'Password')}
                <input
                  className={`${inputCls} mt-1`}
                  type="password"
                  autoComplete="new-password"
                  value={f.password}
                  placeholder={dest?.remote?.type === 'sftp' && dest.remote.has_password ? t('Inchangé (laisser vide)', 'Unchanged (leave empty)') : ''}
                  onChange={(e) => setF((p) => ({ ...p, password: e.target.value }))}
                />
              </label>
            ) : (
              <label className="text-xs font-medium text-muted sm:col-span-2">
                {t('Fichier de clé privée (format OpenSSH, sans phrase de passe)', 'Private key file (OpenSSH format, without passphrase)')}
                <input className={`${inputCls} mt-1 font-mono`} value={f.privateKey} placeholder="/home/user/.ssh/rustnvr_ed25519" spellCheck={false} onChange={(e) => setF((p) => ({ ...p, privateKey: e.target.value }))} />
              </label>
            )}
            <label className="text-xs font-medium text-muted sm:col-span-2">
              {t('Empreinte de la clé du serveur (facultatif)', 'Server key fingerprint (optional)')}
              <input className={`${inputCls} mt-1 font-mono`} value={f.hostKey} placeholder="SHA256:…" spellCheck={false} onChange={(e) => setF((p) => ({ ...p, hostKey: e.target.value }))} />
              <span className="mt-1 block font-normal">
                {t('Sans empreinte, la clé vue à la première connexion est retenue puis exigée (comme ssh). Sur le serveur : ', 'Without a fingerprint, the key seen at the first connection is kept then required (like ssh). On the server: ')}<code>ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub</code>
              </span>
            </label>
          </>
        )}
        <label className="text-xs font-medium text-muted sm:col-span-2">
          {f.kind === 'nas' ? t('Dossier (dans Linux)', 'Folder (in Linux)') : t("Dossier d'attente local (facultatif)", "Local outbox folder (optional)")}
          <input
            className={`${inputCls} mt-1 font-mono`}
            value={f.path}
            placeholder={f.kind === 'nas' ? '/mnt/nas/rustnvr' : t('automatique (dans le dossier de données)', 'automatic (in the data folder)')}
            spellCheck={false}
            onChange={(e) => setF((p) => ({ ...p, path: e.target.value }))}
          />
        </label>
        {f.kind === 'nas' && (
          <label className="flex items-start gap-2 text-sm sm:col-span-2">
            <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={f.requireMount} onChange={(e) => setF((p) => ({ ...p, requireMount: e.target.checked }))} />
            <span>
              {t('Partage réseau monté obligatoire (NAS)', 'Mounted network share required (NAS)')}
              <span className="block text-xs text-muted">
                {t("Recommandé : RustNVR refuse d'écrire si le dossier se trouve sur le disque système, ce qui arrive quand le partage est démonté. Décochez pour un second disque local.", "Recommended: RustNVR refuses to write if the folder is on the system disk, which happens when the share is unmounted. Untick for a second local disk.")}
              </span>
            </span>
          </label>
        )}
        <label className="text-xs font-medium text-muted">
          {t('Conserver (jours)', 'Keep (days)')}
          <input className={`${inputCls} mt-1`} inputMode="numeric" placeholder={t('illimité', 'unlimited')} value={f.days} onChange={(e) => setF((p) => ({ ...p, days: e.target.value }))} />
        </label>
        <label className="text-xs font-medium text-muted">
          {t('Taille max. (Go)', 'Max. size (GB)')}
          <input className={`${inputCls} mt-1`} inputMode="numeric" placeholder={t('illimitée', 'unlimited')} value={f.gb} onChange={(e) => setF((p) => ({ ...p, gb: e.target.value }))} />
        </label>
      </div>
      {error && <p className="mt-4 rounded-md border border-err/40 bg-err/10 px-3 py-2 text-sm text-err">{error}</p>}
      <details className="mt-4 text-xs text-muted">
        <summary className="cursor-pointer hover:text-fg">{t('Comment monter un partage NAS dans Linux / WSL ?', 'How to mount a NAS share in Linux / WSL?')}</summary>
        <div className="mt-2 space-y-2">
          <p>{t('Partage SMB (Windows, Synology, TrueNAS…), dans le terminal Linux :', 'SMB share (Windows, Synology, TrueNAS…), in the Linux terminal:')}</p>
          <pre className="overflow-x-auto rounded border border-line bg-bg-2 p-2 text-[11px] text-fg">
            {`sudo apt install cifs-utils
sudo mkdir -p /mnt/nas
sudo mount -t cifs //192.168.1.10/videos /mnt/nas \\
  -o username=UTILISATEUR,uid=$(id -u),gid=$(id -g),soft
mkdir -p /mnt/nas/rustnvr`}
          </pre>
          <p>{t('Partage NFS : ', 'NFS share: ')}<code>sudo mount -t nfs -o soft 192.168.1.10:/volume1/videos /mnt/nas</code>{t('. Pour un montage permanent, ajoutez-le à /etc/fstab.', '. For a permanent mount, add it to /etc/fstab.')}</p>
        </div>
      </details>
    </Modal>
  )
}

/** Paramètres → Stockage: destinations and retention. */
export function StorageSettings() {
  const demo = useIsDemo()
  const canManage = useCapability('retention') === true
  const viewerReason = useReadOnly()
  const cameras = useLive((s) => s.cameras)
  const names = new Map(cameras.map((c) => [c.id, c.name]))
  const list = useAsync(() => (demo ? Promise.resolve(null) : getDestinations()), [demo])
  const [override, setOverride] = useState<Destination[] | null>(null)
  const [form, setForm] = useState<{ open: boolean; dest: Destination | null }>({ open: false, dest: null })
  const [toDelete, setToDelete] = useState<{ dest: Destination; error: string | null } | null>(null)
  const dests = override ?? list.data
  const canSetDefault = useCapability('settings_write') === true
  const settings = useAsync(() => (demo || !canSetDefault ? Promise.resolve(null) : getSettings()), [demo, canSetDefault])
  const [defaultId, setDefaultId] = useState<string | null>(null)
  const [defaultError, setDefaultError] = useState<string | null>(null)
  const [savingDefault, setSavingDefault] = useState(false)
  const wanted = defaultId ?? settings.data?.default_destination ?? 'local'
  // A removed destination falls back to the local disk (as on the server).
  const effectiveDefault = dests?.some((d) => d.id === wanted) ? wanted : 'local'
  const makeDefault = async (id: string) => {
    setSavingDefault(true)
    setDefaultError(null)
    try {
      // Fresh settings, so that other changes made on this page are kept.
      const s = await getSettings()
      const saved = await setSettings({
        recording: s.recording,
        motion: s.motion,
        live_transcode: s.live_transcode,
        live_max_bitrate_kbps: s.live_max_bitrate_kbps,
        low_space_percent: s.low_space_percent,
        default_destination: id,
      })
      setDefaultId(saved.default_destination)
    } catch (e) {
      setDefaultError(describeError(e))
    } finally {
      setSavingDefault(false)
    }
  }
  const reason = demo ? t('Indisponible en mode démo.', 'Not available in demo mode.') : viewerReason ? viewerReason : !canManage ? t("Ce serveur ne permet pas de modifier le stockage depuis l'interface.", "This server does not allow changing storage from the interface.") : null

  if (demo) {
    return <p className="px-5 py-4 text-sm text-muted">{t("La gestion du stockage n'est pas disponible en mode démo.", "Storage management is not available in demo mode.")}</p>
  }
  if (list.error && !dests) return <ErrorState error={list.error} onRetry={list.reload} compact />
  if (!dests) return <LoadingState compact />

  return (
    <div className="space-y-3 px-5 py-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-sm text-muted">{t("Chaque caméra choisit ses lieux d'enregistrement dans sa fiche (Caméras → Modifier). Le point « par défaut » est proposé pour les nouvelles caméras et utilisé quand on active l'enregistrement sans en choisir.", "Each camera chooses where it records in its settings (Cameras → Edit). The « default » destination is offered for new cameras and used when recording is turned on without choosing one.")}</p>
        <Button variant="primary" size="sm" icon={<Plus className="size-4" />} disabledReason={reason} onClick={() => setForm({ open: true, dest: null })}>
          {t('Ajouter un point de stockage', 'Add a storage destination')}
        </Button>
      </div>
      {defaultError && <p className="text-sm text-err">{t('Point par défaut non modifié : ', 'Default destination not changed: ')}{defaultError}</p>}
      {dests.map((d) => {
        const pct = d.disk && d.disk.total_bytes > 0 ? (100 * d.disk.used_bytes) / d.disk.total_bytes : null
        const editable = d.managed && d.kind !== 'local'
        const fromFile = t('Déclaré dans config.toml : modifiez ce fichier puis redémarrez RustNVR.', 'Declared in config.toml: edit this file then restart RustNVR.')
        return (
          <div key={d.id} className="space-y-3 rounded-md border border-line p-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="flex min-w-0 items-start gap-3">
                {d.kind === 'local' ? <HardDrive className="mt-0.5 size-5 text-muted" aria-hidden /> : d.kind === 'nas' ? <Network className="mt-0.5 size-5 text-muted" aria-hidden /> : <Cloud className="mt-0.5 size-5 text-muted" aria-hidden />}
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2 font-medium">
                    {d.name}
                    {d.id === effectiveDefault && (
                      <Badge tone="info">
                        <Star className="size-3" aria-hidden />{t(' Par défaut', ' Default')}
                      </Badge>
                    )}
                    {d.low_space && <Badge tone="warn">{t('Espace faible', 'Low space')}</Badge>}
                    {d.available ? <Badge tone="ok">{t('Disponible', 'Available')}</Badge> : <Badge tone="err">{t('Indisponible', 'Unavailable')}</Badge>}
                    {!d.managed && <Badge>config.toml</Badge>}
                  </div>
                  <p className="truncate font-mono text-xs text-muted">{d.path}</p>
                  {d.unavailable_reason && <p className="text-xs text-err">{d.unavailable_reason}</p>}
                  {d.upload && <UploadLine dest={d} />}
                  {d.id !== effectiveDefault && (
                    <Button
                      size="sm"
                      variant="ghost"
                      className="-ml-2.5"
                      icon={<Star className="size-3.5" />}
                      disabledReason={demo ? t('Indisponible en mode démo.', 'Not available in demo mode.') : viewerReason ? viewerReason : !canSetDefault ? t("Ce serveur ne permet pas de modifier les réglages depuis l'interface.", "This server does not allow changing the settings from the interface.") : savingDefault ? t('Enregistrement…', 'Saving…') : null}
                      onClick={() => void makeDefault(d.id)}
                    >
                      {t('Utiliser par défaut', 'Use by default')}
                    </Button>
                  )}
                  <p className="text-xs text-muted">
                    {t('Caméras : ', 'Cameras: ')}{d.used_by.length === 0 ? 'aucune' : d.used_by.map((id) => names.get(id) ?? id).join(', ')}
                  </p>
                </div>
              </div>
              {d.kind !== 'local' && (
                <div className="flex gap-1">
                  <IconButton label={t('Modifier', 'Edit')} icon={<Pencil className="size-4" />} disabledReason={reason ?? (editable ? null : fromFile)} onClick={() => setForm({ open: true, dest: d })} />
                  <IconButton
                    label={t('Supprimer', 'Delete')}
                    icon={<Trash className="size-4" />}
                    disabledReason={reason ?? (!editable ? fromFile : d.used_by.length > 0 ? t('Utilisé par des caméras : retirez-le d’abord de leur fiche.', 'Used by cameras: first remove it from their settings.') : null)}
                    onClick={() => setToDelete({ dest: d, error: null })}
                  />
                </div>
              )}
            </div>
            {d.disk && (
              <div className="max-w-md space-y-1">
                <Meter value={pct} label={t(`Occupation de ${d.name}`, `Usage of ${d.name}`)} />
                <p className="text-xs text-muted tabular">
                  {formatPercent(pct)}{t(' utilisés · ', ' used · ')}{formatBytes(d.disk.free_bytes)}{t(' libres sur ', ' free of ')}{formatBytes(d.disk.total_bytes)}
                </p>
              </div>
            )}
            <RetentionEditor key={`${d.id}:${d.retention_days}:${d.max_size_gb}:${d.min_free_gb}`} dest={d} onSaved={setOverride} disabledReason={reason ?? (d.kind === 'local' || editable ? null : fromFile)} />
          </div>
        )
      })}
      <DestinationForm open={form.open} dest={form.dest} onClose={() => setForm({ open: false, dest: null })} onSaved={setOverride} />
      <ConfirmDialog
        open={toDelete !== null}
        title={t('Supprimer ce point de stockage ?', 'Delete this storage destination?')}
        danger
        confirmLabel={t('Supprimer', 'Delete')}
        message={
          toDelete && (
            <>
              <strong className="text-fg">{toDelete.dest.name}</strong>{t(' ne sera plus utilisé. Les vidéos déjà enregistrées dans ', ' will no longer be used. The videos already recorded in ')}{toDelete.dest.path}{t(' ne sont pas supprimées.', ' are not deleted.')}
              {toDelete.error && <span className="mt-2 block text-err">{t('Échec : ', 'Failed: ')}{toDelete.error}</span>}
            </>
          )
        }
        onCancel={() => setToDelete(null)}
        onConfirm={() => {
          if (!toDelete) return
          deleteDestination(toDelete.dest.id).then(
            (l) => {
              setOverride(l)
              setToDelete(null)
            },
            (e: unknown) => setToDelete({ dest: toDelete.dest, error: describeError(e) }),
          )
        }}
      />
    </div>
  )
}

/** Upload state of an S3 / SFTP destination. */
function UploadLine({ dest }: { dest: Destination }) {
  const u = dest.upload
  if (!u) return null
  const target = dest.remote?.type === 's3' ? `S3 ${dest.remote.bucket}` : dest.remote?.type === 'sftp' ? `SFTP ${dest.remote.username}@${dest.remote.host}` : ''
  return (
    <div className="space-y-0.5 text-xs">
      <p className="text-muted">
        {t('Envoi vers ', 'Upload to ')}{target} : {u.uploaded}{t(' segment(s) envoyé(s) (', ' segment(s) sent (')}{formatBytes(u.uploaded_bytes)}{t(') depuis le démarrage', ') since startup')}
        {u.last_upload_ms ? t(`, dernier à ${new Date(u.last_upload_ms).toLocaleTimeString(locale())}`, `, last at ${new Date(u.last_upload_ms).toLocaleTimeString(locale())}`) : ''} · {u.pending}{t(' en attente', ' waiting')}
      </p>
      {u.error && <p className="text-err">{u.error}</p>}
      {u.host_key && <p className="font-mono text-muted">{t('Clé du serveur : ', 'Server key: ')}{u.host_key}</p>}
    </div>
  )
}
