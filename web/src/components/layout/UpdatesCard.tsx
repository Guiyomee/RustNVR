import { ExternalLink, RefreshCw } from 'lucide-react'
import { useEffect, useState } from 'react'
import { describeError } from '../../api/client'
import { checkUpdates, getUpdates, setUpdates } from '../../api/endpoints'
import { useReadOnly } from '../../auth/AuthGate'
import { useIsDemo } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import { formatDateTime } from '../../lib/format'
import type { UpdateStatus } from '../../types/api'
import { Button } from '../ui/Button'
import { Badge, Card, CardHeader } from '../ui/Card'
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

/** À propos → Mises à jour: latest version published on the watched repository. */
export function UpdatesCard() {
  const demo = useIsDemo()
  const readOnly = useReadOnly()
  const loaded = useAsync(() => (demo ? Promise.resolve(null) : getUpdates()), [demo])
  const [u, setU] = useState<UpdateStatus | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (loaded.data) setU(loaded.data)
  }, [loaded.data])

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
              {install && (
                <div>
                  <p className="text-xs font-medium">{install.title}{t(' — pour installer :', ' — to install:')}</p>
                  <pre className="mt-1 overflow-auto rounded bg-bg-2 p-3 font-mono text-xs whitespace-pre-wrap">{install.steps}</pre>
                </div>
              )}
            </div>
          )}

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
          <p className="text-xs text-muted">
            {t('RustNVR ne télécharge ni n’installe rien : il compare sa version à la dernière publication (« release ») du dépôt officiel et indique comment mettre à jour. Un autre dépôt (fork, serveur Gitea) se règle dans config.toml : ', 'RustNVR neither downloads nor installs anything: it compares its version with the latest release of the official repository and tells how to update. Another repository (fork, Gitea server) is set in config.toml: ')}<code className="font-mono">[updates] repository</code>{t(' (jeton d’un dépôt privé : ', ' (token of a private repository: ')}<code className="font-mono">token_env</code>{t(').', ').')}
          </p>
        </div>
      )}
    </Card>
  )
}
