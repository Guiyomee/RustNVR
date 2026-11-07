import { Download, FileSearch, RotateCcw } from 'lucide-react'
import { useState } from 'react'
import { describeError } from '../../api/client'
import { createBackup, inspectBackup, restoreBackup } from '../../api/endpoints'
import { useAuth, useReadOnly } from '../../auth/AuthGate'
import { useIsDemo } from '../../data/DataContext'
import { formatDateTime } from '../../lib/format'
import { t } from '../../lib/i18n'
import type { BackupPreview, BackupRestored } from '../../types/api'
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
}

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
      setMotion(p.motion.length > 0)
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
            {preview.motion.length > 0 && (
              <li>
                <label className="flex items-center gap-2">
                  <input type="checkbox" className="size-4 accent-accent" checked={motion} disabled={done !== null} onChange={(e) => setMotion(e.target.checked)} />
                  <span>{t(`Détection de ${preview.motion.length} caméra(s) : zones, lignes de passage, masques`, `Detection of ${preview.motion.length} camera(s): zones, crossing lines, masks`)}</span>
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
          {t(`Restauré : ${done.files.length} fichier(s)${done.motion ? `, détection de ${done.motion} caméra(s)` : ''}. Redémarrez RustNVR pour l’utiliser (`, `Restored: ${done.files.length} file(s)${done.motion ? `, detection of ${done.motion} camera(s)` : ''}. Restart RustNVR to use it (`)}
          <code className="font-mono">docker restart rustnvr</code>
          {t(' ou ', ' or ')}
          <code className="font-mono">systemctl restart rustnvr</code>).
        </p>
      )}
      {error && <p className="text-sm text-err">{error}</p>}
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
      <RestoreBackup blocked={blocked} />
    </div>
  )
}
