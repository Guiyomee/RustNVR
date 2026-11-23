import { Download, FileSearch, Play, RotateCcw, Save } from 'lucide-react'
import { useEffect, useState } from 'react'
import { describeError } from '../../api/client'
import { createBackup, getAutoBackup, getDestinations, inspectBackup, restoreBackup, runAutoBackup, setAutoBackup } from '../../api/endpoints'
import { useAuth, useReadOnly } from '../../auth/AuthGate'
import { useIsDemo } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { formatBytes, formatDateTime } from '../../lib/format'
import { t } from '../../lib/i18n'
import type { AutoBackupView, BackupPreview, BackupRestored, Destination } from '../../types/api'
import { Button } from '../ui/Button'

const inputCls = 'mt-1 h-9 w-full rounded-md border border-line bg-bg-2 px-3 text-sm text-fg outline-none focus:border-accent disabled:opacity-60'
/** Same rule as the server (`backup::MIN_PASSPHRASE`). */
const MIN_PASSPHRASE = 8

const FILE_LABELS: Record<string, string> = {
  'config.toml': t('Fichier de configuration (config.toml)', 'Configuration file (config.toml)'),
  'cameras.ui.toml': t('Caméras ajoutées dans l’interface, avec leurs mots de passe', 'Cameras added in the interface, with their passwords'),
  'storage.ui.toml': t('Stockage (destinations, rétention)', 'Storage (destinations, retention)'),
  'settings.ui.toml': t('Réglages généraux et réseau', 'General and network settings'),
  'mqtt.ui.toml': 'MQTT / Home Assistant',
  'ai.ui.toml': t('Intelligence artificielle', 'Artificial intelligence'),
  'webhooks.ui.toml': t('Webhooks', 'Webhooks'),
  'updates.ui.toml': t('Mises à jour', 'Updates'),
  'backup.ui.toml': t('Sauvegarde automatique, avec sa phrase secrète', 'Automatic backup, with its passphrase'),
}

/** Same limits as the server (`AUTO_BACKUP_MAX_DAYS`, `AUTO_BACKUP_MAX_KEEP`). */
const EVERY: { days: number; label: string }[] = [
  { days: 1, label: t('Chaque jour', 'Every day') },
  { days: 3, label: t('Tous les 3 jours', 'Every 3 days') },
  { days: 7, label: t('Chaque semaine (conseillé)', 'Every week (advised)') },
  { days: 14, label: t('Toutes les 2 semaines', 'Every 2 weeks') },
  { days: 30, label: t('Tous les 30 jours', 'Every 30 days') },
]
const MAX_KEEP = 52

const STATE_LABELS: Record<BackupPreview['files'][number]['state'], { text: string; tone: string }> = {
  same: { text: t('identique', 'same'), tone: 'text-muted' },
  changed: { text: t('différent', 'different'), tone: 'text-warn' },
  new: { text: t('absent ici', 'missing here'), tone: 'text-accent' },
  not_in_backup: { text: t('pas dans la sauvegarde : gardé', 'not in the backup: kept'), tone: 'text-muted' },
}

function stamp(): string {
  const d = new Date()
  const p = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}`
}

/** The file, base64 (without the `data:` prefix). */
function readBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result).replace(/^data:[^,]*,/, ''))
    reader.onerror = () => reject(new Error(t('Fichier illisible', 'Unreadable file')))
    reader.readAsDataURL(file)
  })
}

function CreateBackup({ blocked }: { blocked: string | null }) {
  const [pass, setPass] = useState('')
  const [again, setAgain] = useState('')
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null)
  const reason =
    blocked ??
    (pass.length < MIN_PASSPHRASE
      ? t(`Phrase secrète : ${MIN_PASSPHRASE} caractères au moins.`, `Passphrase: ${MIN_PASSPHRASE} characters at least.`)
      : pass !== again
        ? t('Les deux phrases secrètes diffèrent.', 'The two passphrases differ.')
        : busy
          ? t('Sauvegarde en cours…', 'Backing up…')
          : null)
  const run = async () => {
    setBusy(true)
    setResult(null)
    try {
      const blob = await createBackup(pass)
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = `rustnvr-config-${stamp()}.rnvrbak`
      a.click()
      URL.revokeObjectURL(url)
      setResult({ ok: true, text: t('Sauvegarde téléchargée. Gardez la phrase secrète : sans elle, le fichier est illisible.', 'Backup downloaded. Keep the passphrase: without it, the file cannot be read.') })
      setPass('')
      setAgain('')
    } catch (e) {
      setResult({ ok: false, text: describeError(e) })
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="space-y-3">
      <h3 className="text-sm font-medium">{t('Créer une sauvegarde', 'Create a backup')}</h3>
      <p className="text-xs text-muted">
        {t(
          'Un fichier chiffré avec la configuration : config.toml, tout ce qui a été réglé dans l’interface (caméras et leurs mots de passe, stockage, MQTT, IA, webhooks…) et la détection de chaque caméra (zones, lignes de passage, masques). Ni les enregistrements, ni les événements, ni les comptes.',
          'An encrypted file with the configuration: config.toml, everything set in the interface (cameras and their passwords, storage, MQTT, AI, webhooks…) and the detection of each camera (zones, crossing lines, masks). Neither recordings, nor events, nor accounts.',
        )}
      </p>
      <div className="grid gap-3 sm:grid-cols-2">
        <label className="text-sm">
          {t('Phrase secrète', 'Passphrase')}
          <input type="password" autoComplete="new-password" className={inputCls} value={pass} disabled={blocked !== null} onChange={(e) => setPass(e.target.value)} />
        </label>
        <label className="text-sm">
          {t('La même, encore', 'The same, again')}
          <input type="password" autoComplete="new-password" className={inputCls} value={again} disabled={blocked !== null} onChange={(e) => setAgain(e.target.value)} />
        </label>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="primary" icon={<Download className="size-4" />} disabledReason={reason} onClick={() => void run()}>
          {t('Télécharger la sauvegarde', 'Download the backup')}
        </Button>
        {result && <span className={`text-xs ${result.ok ? 'text-ok' : 'text-err'}`}>{result.text}</span>}
      </div>
    </div>
  )
}

function RestoreBackup({ blocked }: { blocked: string | null }) {
  const [file, setFile] = useState<File | null>(null)
  const [pass, setPass] = useState('')
  const [data, setData] = useState<string | null>(null)
  const [preview, setPreview] = useState<BackupPreview | null>(null)
  const [chosen, setChosen] = useState<string[]>([])
  const [motion, setMotion] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [done, setDone] = useState<BackupRestored | null>(null)

  const reset = () => {
    setPreview(null)
    setData(null)
    setDone(null)
    setError(null)
  }
  const analyse = async () => {
    if (!file) return
    setBusy(true)
    reset()
    try {
      const encoded = await readBase64(file)
      const p = await inspectBackup(encoded, pass)
      setData(encoded)
      setPreview(p)
      // Not ticked by default: a file that would not load here, and
      // config.toml (its paths and port belong to the installation).
      setChosen(p.files.filter((f) => (f.state === 'changed' || f.state === 'new') && !f.problem && f.name !== 'config.toml').map((f) => f.name))
      setMotion(p.motion.length > 0 || Boolean(p.wall) || (p.journeys ?? 0) > 0)
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }
  const restore = async () => {
    if (!data) return
    setBusy(true)
    setError(null)
    try {
      setDone(await restoreBackup(data, pass, chosen, motion))
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }
  const analyseReason = blocked ?? (!file ? t('Choisissez un fichier .rnvrbak.', 'Choose a .rnvrbak file.') : !pass ? t('Phrase secrète de la sauvegarde manquante.', 'Passphrase of the backup missing.') : busy ? t('Lecture en cours…', 'Reading…') : null)
  const restoreReason =
    blocked ??
    (chosen.length === 0 && !motion
        ? t('Rien de coché.', 'Nothing ticked.')
        : busy
          ? t('Restauration en cours…', 'Restoring…')
          : done
            ? t('Déjà restaurée : redémarrez RustNVR.', 'Already restored: restart RustNVR.')
            : null)

  return (
    <div className="space-y-3">
      <h3 className="text-sm font-medium">{t('Restaurer une sauvegarde', 'Restore a backup')}</h3>
      <div className="grid gap-3 sm:grid-cols-2">
        <label className="text-sm">
          {t('Fichier', 'File')}
          <input
            type="file"
            accept=".rnvrbak"
            className="mt-1 block w-full text-sm text-muted file:mr-3 file:rounded-md file:border file:border-line file:bg-bg-2 file:px-3 file:py-1.5 file:text-fg"
            disabled={blocked !== null}
            onChange={(e) => {
              setFile(e.target.files?.[0] ?? null)
              reset()
            }}
          />
        </label>
        <label className="text-sm">
          {t('Phrase secrète de la sauvegarde', 'Passphrase of the backup')}
          <input
            type="password"
            autoComplete="off"
            className={inputCls}
            value={pass}
            disabled={blocked !== null}
            onChange={(e) => {
              setPass(e.target.value)
              reset()
            }}
          />
        </label>
      </div>
      <Button icon={<FileSearch className="size-4" />} disabledReason={analyseReason} onClick={() => void analyse()}>
        {t('Analyser', 'Analyse')}
      </Button>

      {preview && (
        <div className="space-y-3 rounded-md border border-line p-4">
          <p className="text-xs text-muted">
            {t(`Sauvegarde du ${formatDateTime(preview.created_ms)}, RustNVR ${preview.rustnvr}. Cochez ce qu’il faut restaurer : les fichiers cochés remplacent ceux d’ici.`, `Backup of ${formatDateTime(preview.created_ms)}, RustNVR ${preview.rustnvr}. Tick what to restore: the ticked files replace the ones here.`)}
          </p>
          <ul className="space-y-1.5 text-sm">
            {preview.files.map((f) => {
              const restorable = f.state === 'changed' || f.state === 'new'
              const state = STATE_LABELS[f.state]
              return (
                <li key={f.name}>
                  <label className={`flex items-center gap-2 ${restorable ? '' : 'opacity-70'}`}>
                    <input
                      type="checkbox"
                      className="size-4 accent-accent"
                      checked={chosen.includes(f.name)}
                      disabled={!restorable || done !== null}
                      onChange={() => setChosen((c) => (c.includes(f.name) ? c.filter((x) => x !== f.name) : [...c, f.name]))}
                    />
                    <span>{FILE_LABELS[f.name] ?? f.name}</span>
                    <span className={`text-xs ${state.tone}`}>· {state.text}</span>
                  </label>
                  {restorable && f.name === 'config.toml' && (
                    <p className="ml-6 text-xs text-warn">
                      {t('Propre à chaque installation (dossiers, port, HTTPS) : à ne restaurer que sur la même installation, pas d’un Docker vers un LXC par exemple.', 'Specific to each installation (folders, port, HTTPS): restore it only on the same installation, not from Docker to an LXC for instance.')}
                    </p>
                  )}
                  {f.problem && <p className="ml-6 whitespace-pre-line text-xs text-err">{f.problem}</p>}
                </li>
              )
            })}
            {(preview.motion.length > 0 || preview.wall || (preview.journeys ?? 0) > 0) && (
              <li>
                <label className="flex items-center gap-2">
                  <input type="checkbox" className="size-4 accent-accent" checked={motion} disabled={done !== null} onChange={(e) => setMotion(e.target.checked)} />
                  <span>
                    {[
                      preview.motion.length > 0 && t(`Détection de ${preview.motion.length} caméra(s) : zones, lignes de passage, masques`, `Detection of ${preview.motion.length} camera(s): zones, crossing lines, masks`),
                      (preview.journeys ?? 0) > 0 && t(`${preview.journeys} flèche(s) de trajets`, `${preview.journeys} journey arrow(s)`),
                      preview.wall && t('réglages du mur', 'wall settings'),
                    ]
                      .filter(Boolean)
                      .join(' · ')}
                  </span>
                </label>
              </li>
            )}
          </ul>
          {preview.cameras.length > 0 && (
            <p className="text-xs text-muted">
              {t('Caméras après une restauration complète : ', 'Cameras after a full restore: ')}
              {preview.cameras.map((c) => c.name).join(', ')}
            </p>
          )}
          {preview.problems.length > 0 && !preview.files.some((f) => f.problem) && (
            <ul className="space-y-1 text-xs text-err">
              <li>{t('En restaurant tous les fichiers ensemble :', 'Restoring every file together:')}</li>
              {preview.problems.map((p) => (
                <li key={p}>{p}</li>
              ))}
            </ul>
          )}
          <Button variant="primary" icon={<RotateCcw className="size-4" />} disabledReason={restoreReason} onClick={() => void restore()}>
            {t('Restaurer la sélection', 'Restore the selection')}
          </Button>
        </div>
      )}
      {done && (
        <p className="text-sm text-ok">
          {t(
            `Restauré : ${done.files.length} fichier(s)${done.motion ? `, détection de ${done.motion} caméra(s)` : ''}${done.journeys ? `, ${done.journeys} flèche(s) de trajets` : ''}${done.wall ? ', réglages du mur' : ''}. Redémarrez RustNVR pour l’utiliser (`,
            `Restored: ${done.files.length} file(s)${done.motion ? `, detection of ${done.motion} camera(s)` : ''}${done.journeys ? `, ${done.journeys} journey arrow(s)` : ''}${done.wall ? ', wall settings' : ''}. Restart RustNVR to use it (`,
          )}
          <code className="font-mono">docker restart rustnvr</code>
          {t(' ou ', ' or ')}
          <code className="font-mono">systemctl restart rustnvr</code>).
        </p>
      )}
      {error && <p className="text-sm text-err">{error}</p>}
    </div>
  )
}

function AutoBackup({ blocked }: { blocked: string | null }) {
  // Nothing is asked of the server when blocked (demo, viewers).
  const loaded = useAsync(
    async () => (blocked ? null : { view: await getAutoBackup(), destinations: await getDestinations() }),
    [blocked],
  )
  const [view, setView] = useState<AutoBackupView | null>(null)
  const [enabled, setEnabled] = useState(false)
  const [destination, setDestination] = useState('')
  const [every, setEvery] = useState(7)
  const [keep, setKeep] = useState(4)
  const [pass, setPass] = useState('')
  const [again, setAgain] = useState('')
  const [busy, setBusy] = useState<'save' | 'run' | null>(null)
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null)

  const show = (v: AutoBackupView) => {
    setView(v)
    setEnabled(v.enabled)
    setDestination(v.destination)
    setEvery(v.every_days)
    setKeep(v.keep)
    setPass('')
    setAgain('')
  }
  useEffect(() => {
    if (loaded.data) show(loaded.data.view)
  }, [loaded.data])

  // The folder of a remote destination is only an outbox.
  const destinations: Destination[] = (loaded.data?.destinations ?? []).filter((d) => d.kind === 'local' || d.kind === 'nas')
  const everyOptions = EVERY.some((e) => e.days === every) ? EVERY : [...EVERY, { days: every, label: t(`Tous les ${every} jours`, `Every ${every} days`) }]
  const dirty = view !== null && (enabled !== view.enabled || destination !== view.destination || every !== view.every_days || keep !== view.keep || pass !== '')
  const needsPass = enabled && !view?.has_passphrase
  const saveReason =
    blocked ??
    (!view
      ? t('Chargement…', 'Loading…')
      : !dirty
        ? t('Rien de changé.', 'Nothing changed.')
        : enabled && !destination
          ? t('Choisissez où écrire les sauvegardes.', 'Choose where to write the backups.')
          : (needsPass || pass !== '') && pass.length < MIN_PASSPHRASE
            ? t(`Phrase secrète : ${MIN_PASSPHRASE} caractères au moins.`, `Passphrase: ${MIN_PASSPHRASE} characters at least.`)
            : pass !== again
              ? t('Les deux phrases secrètes diffèrent.', 'The two passphrases differ.')
              : !Number.isInteger(keep) || keep < 1 || keep > MAX_KEEP
                ? t(`Sauvegardes gardées : de 1 à ${MAX_KEEP}.`, `Backups kept: 1 to ${MAX_KEEP}.`)
                : busy
                  ? t('Patientez…', 'Please wait…')
                  : null)
  const runReason =
    blocked ??
    (!view
      ? t('Chargement…', 'Loading…')
      : dirty
        ? t('Enregistrez d’abord les réglages.', 'Save the settings first.')
        : !view.destination
          ? t('Choisissez et enregistrez où écrire les sauvegardes.', 'Choose and save where to write the backups.')
          : !view.has_passphrase
            ? t('Enregistrez d’abord une phrase secrète.', 'Save a passphrase first.')
            : busy
              ? t('Patientez…', 'Please wait…')
              : null)

  const save = async () => {
    setBusy('save')
    setMessage(null)
    try {
      show(await setAutoBackup({ enabled, destination, every_days: every, keep, ...(pass ? { passphrase: pass } : {}) }))
      setMessage({ ok: true, text: t('Enregistré.', 'Saved.') })
    } catch (e) {
      setMessage({ ok: false, text: describeError(e) })
    } finally {
      setBusy(null)
    }
  }
  const run = async () => {
    setBusy('run')
    setMessage(null)
    try {
      const r = await runAutoBackup()
      setMessage({ ok: true, text: t(`Sauvegarde écrite : ${r.file ?? ''}`, `Backup written: ${r.file ?? ''}`) })
    } catch (e) {
      setMessage({ ok: false, text: describeError(e) })
    }
    try {
      show(await getAutoBackup())
    } catch {
      // The message above says enough.
    }
    setBusy(null)
  }

  return (
    <div className="space-y-3">
      <h3 className="text-sm font-medium">{t('Sauvegarde automatique', 'Automatic backup')}</h3>
      <p className="text-xs text-muted">
        {t(
          'RustNVR fait lui-même cette sauvegarde chiffrée à intervalle régulier, dans le dossier .rustnvr-backups du stockage choisi (de préférence le NAS : elle survit à la perte du disque du serveur), et ne garde que les dernières. La phrase secrète est enregistrée sur le serveur (backup.ui.toml, à côté de config.toml) : notez-la aussi ailleurs, sans elle les fichiers sont illisibles.',
          'RustNVR makes this encrypted backup by itself at regular intervals, in the .rustnvr-backups folder of the chosen storage (the NAS, preferably: it survives the loss of the server’s disk), and keeps only the last ones. The passphrase is stored on the server (backup.ui.toml, next to config.toml): write it down elsewhere too, without it the files cannot be read.',
        )}
      </p>
      {loaded.error !== null && <p className="text-sm text-err">{describeError(loaded.error)}</p>}
      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" className="size-4 accent-accent" checked={enabled} disabled={blocked !== null || !view} onChange={(e) => setEnabled(e.target.checked)} />
        {t('Faire une sauvegarde automatique', 'Make an automatic backup')}
      </label>
      <div className="grid gap-3 sm:grid-cols-3">
        <label className="text-sm">
          {t('Où', 'Where')}
          <select className={inputCls} value={destination} disabled={blocked !== null || !view} onChange={(e) => setDestination(e.target.value)}>
            <option value="">{t('— choisir —', '— choose —')}</option>
            {destinations.map((d) => (
              <option key={d.id} value={d.id}>
                {d.name}
                {d.available ? '' : t(' (indisponible)', ' (unavailable)')}
              </option>
            ))}
          </select>
        </label>
        <label className="text-sm">
          {t('Fréquence', 'How often')}
          <select className={inputCls} value={every} disabled={blocked !== null || !view} onChange={(e) => setEvery(Number(e.target.value))}>
            {everyOptions.map((o) => (
              <option key={o.days} value={o.days}>
                {o.label}
              </option>
            ))}
          </select>
        </label>
        <label className="text-sm">
          {t('Sauvegardes gardées', 'Backups kept')}
          <input type="number" min={1} max={MAX_KEEP} className={inputCls} value={keep} disabled={blocked !== null || !view} onChange={(e) => setKeep(Number(e.target.value))} />
        </label>
      </div>
      <div className="grid gap-3 sm:grid-cols-2">
        <label className="text-sm">
          {view?.has_passphrase ? t('Nouvelle phrase secrète (vide : inchangée)', 'New passphrase (empty: unchanged)') : t('Phrase secrète', 'Passphrase')}
          <input type="password" autoComplete="new-password" className={inputCls} value={pass} disabled={blocked !== null || !view} onChange={(e) => setPass(e.target.value)} />
        </label>
        <label className="text-sm">
          {t('La même, encore', 'The same, again')}
          <input type="password" autoComplete="new-password" className={inputCls} value={again} disabled={blocked !== null || !view} onChange={(e) => setAgain(e.target.value)} />
        </label>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="primary" icon={<Save className="size-4" />} disabledReason={saveReason} onClick={() => void save()}>
          {t('Enregistrer', 'Save')}
        </Button>
        <Button icon={<Play className="size-4" />} disabledReason={runReason} onClick={() => void run()}>
          {busy === 'run' ? t('Sauvegarde…', 'Backing up…') : t('Sauvegarder maintenant', 'Back up now')}
        </Button>
        {message && <span className={`text-xs ${message.ok ? 'text-ok' : 'text-err'}`}>{message.text}</span>}
      </div>
      {view && view.destination && (
        <div className="space-y-1 text-xs text-muted">
          {view.folder && (
            <p>
              {t('Dossier : ', 'Folder: ')}
              <code className="font-mono">{view.folder}</code>
            </p>
          )}
          {view.folder_problem && <p className="text-err">{view.folder_problem}</p>}
          {view.last?.error && (
            <p className="text-err">
              {t(`Échec le ${formatDateTime(view.last.at_ms)} : `, `Failed on ${formatDateTime(view.last.at_ms)}: `)}
              {view.last.error}
            </p>
          )}
          {view.next_ms !== null && <p>{t(`Prochaine : vers le ${formatDateTime(view.next_ms)}`, `Next: around ${formatDateTime(view.next_ms)}`)}</p>}
          {view.backups.length > 0 ? (
            <ul className="space-y-0.5">
              {view.backups.map((b) => (
                <li key={b.name}>
                  <span className="font-mono">{b.name}</span> · {formatBytes(b.bytes)} · {formatDateTime(b.created_ms)}
                </li>
              ))}
            </ul>
          ) : (
            <p>{t('Aucune sauvegarde dans ce dossier pour l’instant.', 'No backup in this folder yet.')}</p>
          )}
        </div>
      )}
    </div>
  )
}

/** Paramètres → Sauvegarde. */
export function BackupSettings() {
  const demo = useIsDemo()
  const { user } = useAuth()
  const readOnly = useReadOnly()
  const blocked = demo
    ? t('Non disponible en mode démo.', 'Not available in demo mode.')
    : readOnly || (user && user.role !== 'admin')
      ? t('Réservé aux administrateurs.', 'Administrators only.')
      : null
  return (
    <div className="space-y-8 px-5 py-4">
      <CreateBackup blocked={blocked} />
      <AutoBackup blocked={blocked} />
      <RestoreBackup blocked={blocked} />
    </div>
  )
}
