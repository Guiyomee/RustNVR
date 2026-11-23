import { Download, ExternalLink, RefreshCw } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { describeError } from '../../api/client'
import { checkUpdates, getUpdates, installUpdate, setUpdates } from '../../api/endpoints'
import { useReadOnly } from '../../auth/AuthGate'
import { useIsDemo } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { formatDateTime } from '../../lib/format'
import type { UpdateStatus } from '../../types/api'
import { Button } from '../ui/Button'
import { Badge, Card, CardHeader } from '../ui/Card'
import { ConfirmDialog } from '../ui/Modal'
import { Switch } from '../ui/Switch'
import { ErrorState, LoadingState } from '../ui/States'
import { t } from '../../lib/i18n'

/** How to install the new version, depending on how RustNVR runs. */
const INSTALL: Record<UpdateStatus['install'], { title: string; steps: string }> = {
  docker: {
    title: t('RustNVR tourne sous Docker', 'RustNVR runs in Docker'),
    steps:
      t("Image publiée (ghcr.io/guiyomee/rustnvr), dans le dossier de compose.yaml :\n  docker compose pull && docker compose up -d\nPortainer : Stacks → rustnvr → « Update the stack » avec « Re-pull image ».\nImage construite depuis le code : « Pull and redeploy » en forçant la reconstruction, ou :\n  git pull && docker compose up -d --build", "Published image (ghcr.io/guiyomee/rustnvr), in the folder of compose.yaml:\n  docker compose pull && docker compose up -d\nPortainer: Stacks → rustnvr → « Update the stack » with « Re-pull image ».\nImage built from the code: « Pull and redeploy » forcing the rebuild, or:\n  git pull && docker compose up -d --build"),
  },
  systemd: {
    title: t('RustNVR tourne comme service systemd', 'RustNVR runs as a systemd service'),
    steps:
      t("Installé avec l’archive (install.sh, conteneur Proxmox) :\n  sudo rustnvr-update\n(Proxmox, depuis l’hôte : pct exec <ctid> -- /usr/local/bin/rustnvr-update)\nCompilé depuis le code (git pull d’abord), voir la page Debian / Ubuntu du wiki :\n  cargo build --release && (cd web && npm ci && npm run build)\n  sudo systemctl stop rustnvr\n  sudo install -m 0755 target/release/rustnvr /usr/local/bin/rustnvr\n  sudo rm -rf /usr/local/share/rustnvr/web && sudo cp -r web/dist /usr/local/share/rustnvr/web\n  sudo systemctl start rustnvr", "Installed from the archive (install.sh, Proxmox container):\n  sudo rustnvr-update\n(Proxmox, from the host: pct exec <ctid> -- /usr/local/bin/rustnvr-update)\nBuilt from the code (git pull first), see the Debian / Ubuntu page of the wiki:\n  cargo build --release && (cd web && npm ci && npm run build)\n  sudo systemctl stop rustnvr\n  sudo install -m 0755 target/release/rustnvr /usr/local/bin/rustnvr\n  sudo rm -rf /usr/local/share/rustnvr/web && sudo cp -r web/dist /usr/local/share/rustnvr/web\n  sudo systemctl start rustnvr"),
  },
  other: {
    title: t('Installation manuelle', 'Manual installation'),
    steps: t('Récupérez le nouveau code (git pull), recompilez (cargo build --release, puis npm ci && npm run build dans web/), installez le programme et redémarrez RustNVR.', 'Fetch the new code (git pull), rebuild (cargo build --release, then npm ci && npm run build in web/), install the program and restart RustNVR.'),
  },
}

function Status({ u }: { u: UpdateStatus }) {
  if (!u.repository) return <Badge>{t('Désactivée', 'Off')}</Badge>
  if (u.error) return <Badge tone="err">{t('Vérification impossible', 'Check failed')}</Badge>
  if (u.checked_at_ms === null) return <Badge>{t('Pas encore vérifiée', 'Not checked yet')}</Badge>
  if (!u.latest) return <Badge>{t('Aucune version publiée', 'No version published')}</Badge>
  if (u.available) return <Badge tone="warn">{t('Nouvelle version ', 'New version ')}{u.latest}</Badge>
  return <Badge tone="ok">{t('À jour', 'Up to date')}</Badge>
}

/** How often the state of an update is read while it runs. */
const WATCH_MS = 4000

/** An update asked from the interface: under way, done or failed. */
function InstallProgress({ u, finished }: { u: UpdateStatus; finished: boolean }) {
  const s = u.install_state
  if (finished) {
    return <p className="rounded-md border border-ok/40 bg-ok/10 p-3 text-ok">{t(`Mise à jour vers ${u.current} terminée : la page se recharge…`, `Update to ${u.current} finished: the page reloads…`)}</p>
  }
  if (!s) return null
  if (s.state === 'requested' || s.state === 'running') {
    return (
      <p className="flex items-center gap-2 rounded-md border border-accent/40 bg-accent/10 p-3">
        <RefreshCw className="size-4 animate-spin" aria-hidden />
        {t(`Mise à jour vers ${s.version ?? '?'} en cours : RustNVR va redémarrer (une minute environ). Cette page se rechargera toute seule.`, `Update to ${s.version ?? '?'} under way: RustNVR will restart (about a minute). This page will reload by itself.`)}
      </p>
    )
  }
  if (s.state === 'failed') {
    return (
      <div className="space-y-1 rounded-md border border-err/40 bg-err/10 p-3">
        <p className="text-err">
          {t('La dernière mise à jour a échoué', 'The last update failed')}
          {s.version && ` (${s.version})`}
          {s.at_ms !== null && <span className="text-xs text-muted"> — {formatDateTime(s.at_ms)}</span>}
        </p>
        {s.message && <pre className="max-h-48 overflow-auto rounded bg-bg-2 p-2 font-mono text-xs whitespace-pre-wrap">{s.message}</pre>}
        <p className="text-xs text-muted">{t('RustNVR continue avec sa version actuelle. Journal complet : /var/lib/rustnvr-update/log.', 'RustNVR goes on with its current version. Full log: /var/lib/rustnvr-update/log.')}</p>
      </div>
    )
  }
  if (s.state === 'done' && s.version && s.at_ms !== null) {
    return <p className="text-xs text-muted">{t(`Dernière mise à jour depuis l’interface : ${s.version}, le ${formatDateTime(s.at_ms)}.`, `Last update from the interface: ${s.version}, on ${formatDateTime(s.at_ms)}.`)}</p>
  }
  return null
}

/** À propos → Mises à jour: latest version published on the watched repository. */
export function UpdatesCard() {
  const demo = useIsDemo()
  const readOnly = useReadOnly()
  const loaded = useAsync(() => (demo ? Promise.resolve(null) : getUpdates()), [demo])
  const [u, setU] = useState<UpdateStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [confirm, setConfirm] = useState(false)
  const [finished, setFinished] = useState(false)
  // The version running when the page opened: once it changes, the update
  // is done and the page reloads (its interface changed too).
  const startedOn = useRef<string | null>(null)
  useEffect(() => {
    if (loaded.data) {
      setU(loaded.data)
      startedOn.current ??= loaded.data.current
    }
  }, [loaded.data])
  const underWay = u?.install_state?.state === 'requested' || u?.install_state?.state === 'running'
  // While an update runs, read its state again; RustNVR restarting does not
  // answer for a while: that is expected.
  useEffect(() => {
    if (!underWay || finished) return
    const timer = window.setInterval(() => {
      getUpdates().then(
        (next) => {
          setU(next)
          if (startedOn.current && next.current !== startedOn.current) {
            setFinished(true)
            window.setTimeout(() => window.location.reload(), 3000)
          }
        },
        () => undefined,
      )
    }, WATCH_MS)
    return () => window.clearInterval(timer)
  }, [underWay, finished])

  if (demo) return null
  const run = async (action: () => Promise<UpdateStatus>) => {
    setBusy(true)
    setError(null)
    try {
      const next = await action()
      setU(next)
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }
  const install = u ? INSTALL[u.install] : null

  return (
    <Card className="lg:col-span-2">
      <CardHeader title={t('Mises à jour', 'Updates')} subtitle={t('Dernière version publiée sur le dépôt surveillé, vérifiée au démarrage puis toutes les 12 heures', 'Latest version published on the watched repository, checked at startup then every 12 hours')} />
      {loaded.error && !u ? (
        <ErrorState compact error={loaded.error} onRetry={loaded.reload} />
      ) : !u ? (
        <LoadingState compact />
      ) : (
        <div className="space-y-4 px-5 py-4 text-sm">
          <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
            <span>
              <span className="text-muted">{t('Installée : ', 'Installed: ')}</span>
              {u.current}
            </span>
            <span>
              <span className="text-muted">{t('Dernière publiée : ', 'Latest published: ')}</span>
              {u.latest ?? '—'}
              {u.published_at && <span className="text-muted"> ({formatDateTime(Date.parse(u.published_at))})</span>}
            </span>
            <Status u={u} />
            {u.checked_at_ms !== null && <span className="text-xs text-muted">{t('Vérifié le ', 'Checked on ')}{formatDateTime(u.checked_at_ms)}</span>}
          </div>
          {u.error && <p className="text-err">{u.error}</p>}

          {u.available && (
            <div className="space-y-3 rounded-md border border-warn/40 bg-warn/5 p-4">
              <p className="font-medium">
                {u.name ?? `RustNVR ${u.latest}`}
                {u.url && (
                  <a className="ml-2 inline-flex items-center gap-1 text-xs text-accent hover:underline" href={u.url} target="_blank" rel="noopener noreferrer">
                    {t('Voir sur le dépôt ', 'See on the repository ')}<ExternalLink className="size-3" aria-hidden />
                  </a>
                )}
              </p>
              {u.notes && <pre className="max-h-64 overflow-auto rounded bg-bg-2 p-3 font-sans text-xs whitespace-pre-wrap">{u.notes}</pre>}
              {u.can_install && u.latest && (
                <Button
                  variant="primary"
                  icon={<Download className="size-4" />}
                  disabledReason={readOnly ?? (underWay ? t('Mise à jour déjà en cours.', 'Update already under way.') : busy ? t('En cours…', 'In progress…') : null)}
                  onClick={() => setConfirm(true)}
                >
                  {t(`Mettre à jour vers ${u.latest}`, `Update to ${u.latest}`)}
                </Button>
              )}
              {install && u.install === 'systemd' && !u.can_install && (
                <p className="text-xs text-muted">
                  {t('La mise à jour en un clic sera proposée ici une fois son service installé. Il manque si la dernière mise à jour s’est faite avec un rustnvr-update d’avant la 2026.11.10 : réinstallez une fois la version actuelle, en root (Proxmox : ', 'One-click updates will be offered here once their service is installed. It is missing when the last update was made with a rustnvr-update older than 2026.11.10: reinstall the current version once, as root (Proxmox: ')}
                  <code className="font-mono">pct exec &lt;ctid&gt; -- /usr/local/bin/rustnvr-update --version v{u.current}</code>
                  {t(' ; Debian : ', '; Debian: ')}
                  <code className="font-mono">sudo rustnvr-update --version v{u.current}</code>).
                </p>
              )}
              {install && (
                <details open={!u.can_install}>
                  <summary className="cursor-pointer text-xs font-medium">{install.title}{u.can_install ? t(' — ou à la main :', ' — or by hand:') : t(' — pour installer :', ' — to install:')}</summary>
                  <pre className="mt-1 overflow-auto rounded bg-bg-2 p-3 font-mono text-xs whitespace-pre-wrap">{install.steps}</pre>
                </details>
              )}
            </div>
          )}
          <InstallProgress u={u} finished={finished} />

          <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
            <label className="flex items-center gap-2">
              <Switch
                checked={u.repository !== null}
                onChange={(on) => void run(() => setUpdates(on))}
                label={t('Vérifier les nouvelles versions', 'Check for new versions')}
                busy={busy}
                disabledReason={readOnly}
              />
              <span>{t('Vérifier les nouvelles versions', 'Check for new versions')}</span>
            </label>
            {u.repository && (
              <a className="inline-flex items-center gap-1 text-xs text-muted hover:text-fg hover:underline" href={u.repository} target="_blank" rel="noopener noreferrer">
                {u.repository.replace(/^https?:\/\//, '')}
                <ExternalLink className="size-3" aria-hidden />
              </a>
            )}
            <Button icon={<RefreshCw className={`size-4 ${busy ? 'animate-spin' : ''}`} />} disabledReason={readOnly ?? (busy ? t('En cours…', 'In progress…') : !u.repository ? t('Vérification désactivée', 'Check turned off') : null)} onClick={() => void run(checkUpdates)}>
              {t('Vérifier maintenant', 'Check now')}
            </Button>
          </div>
          {error && <p className="text-err">{error}</p>}
          <ConfirmDialog
            open={confirm}
            title={t(`Mettre à jour vers ${u.latest ?? ''} ?`, `Update to ${u.latest ?? ''}?`)}
            confirmLabel={t('Mettre à jour', 'Update')}
            message={t(
              'RustNVR télécharge la nouvelle version, vérifie son empreinte, s’arrête puis redémarre : environ une minute sans enregistrement. La configuration, les réglages et les enregistrements sont gardés. Si la nouvelle version modifie la base de données (voir ses notes), une version plus ancienne ne pourra plus la rouvrir.',
              'RustNVR downloads the new version, checks its fingerprint, stops then starts again: about a minute without recording. The configuration, the settings and the recordings are kept. If the new version changes the database (see its notes), an older version can no longer open it.',
            )}
            onCancel={() => setConfirm(false)}
            onConfirm={() => {
              setConfirm(false)
              if (u.latest) {
                const version = u.latest
                void run(() => installUpdate(version))
              }
            }}
          />
          <p className="text-xs text-muted">
            {t('Installé avec install.sh (Debian, Proxmox), RustNVR peut se mettre à jour depuis cette page : un service séparé, en root, télécharge la version demandée sur le dépôt choisi à l’installation et vérifie son empreinte. Sous Docker, il compare seulement sa version et indique comment mettre à jour. Un autre dépôt (fork, serveur Gitea) se règle dans config.toml : ', 'Installed with install.sh (Debian, Proxmox), RustNVR can update itself from this page: a separate service, as root, downloads the version asked from the repository chosen at installation and checks its fingerprint. In Docker, it only compares its version and tells how to update. Another repository (fork, Gitea server) is set in config.toml: ')}<code className="font-mono">[updates] repository</code>{t(' (jeton d’un dépôt privé : ', ' (token of a private repository: ')}<code className="font-mono">token_env</code>{t(').', ').')}
          </p>
        </div>
      )}
    </Card>
  )
}
