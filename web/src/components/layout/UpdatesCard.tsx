import { ExternalLink, RefreshCw, Save } from 'lucide-react'
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
import { ErrorState, LoadingState } from '../ui/States'
import { t } from '../../lib/i18n'

/** How to install the new version, depending on how RustNVR runs. */
const INSTALL: Record<UpdateStatus['install'], { title: string; steps: string }> = {
  docker: {
    title: t('RustNVR tourne sous Docker', 'RustNVR runs in Docker'),
    steps:
      t("Portainer : Stacks → rustnvr → « Pull and redeploy » (en forçant la reconstruction).\nOu sur la machine, dans le dossier du dépôt :\n  git pull && docker compose up -d --build", "Portainer: Stacks → rustnvr → « Pull and redeploy » (forcing the rebuild).\nOr on the machine, in the repository folder:\n  git pull && docker compose up -d --build"),
  },
  systemd: {
    title: t('RustNVR tourne comme service systemd', 'RustNVR runs as a systemd service'),
    steps:
      t("Dans le dossier du code (git pull d’abord), voir docs/INSTALLATION.md § 10 :\n  cargo build --release && (cd web && npm ci && npm run build)\n  sudo systemctl stop rustnvr\n  sudo install -m 0755 target/release/rustnvr /usr/local/bin/rustnvr\n  sudo rm -rf /usr/local/share/rustnvr/web && sudo cp -r web/dist /usr/local/share/rustnvr/web\n  sudo systemctl start rustnvr", "In the code folder (git pull first), see docs/INSTALLATION.md § 10:\n  cargo build --release && (cd web && npm ci && npm run build)\n  sudo systemctl stop rustnvr\n  sudo install -m 0755 target/release/rustnvr /usr/local/bin/rustnvr\n  sudo rm -rf /usr/local/share/rustnvr/web && sudo cp -r web/dist /usr/local/share/rustnvr/web\n  sudo systemctl start rustnvr"),
  },
  other: {
    title: t('Installation manuelle', 'Manual installation'),
    steps: t('Récupérez le nouveau code (git pull), recompilez (cargo build --release, puis npm ci && npm run build dans web/), installez le programme et redémarrez RustNVR.', 'Fetch the new code (git pull), rebuild (cargo build --release, then npm ci && npm run build in web/), install the program and restart RustNVR.'),
  },
}

function Status({ u }: { u: UpdateStatus }) {
  if (!u.repository) return <Badge>{t('Non configurée', 'Not configured')}</Badge>
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
  const [repository, setRepository] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (loaded.data) {
      setU(loaded.data)
      setRepository(loaded.data.repository ?? '')
    }
  }, [loaded.data])

  if (demo) return null
  const run = async (action: () => Promise<UpdateStatus>) => {
    setBusy(true)
    setError(null)
    try {
      const next = await action()
      setU(next)
      setRepository(next.repository ?? '')
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }
  const changed = u !== null && repository.trim() !== (u.repository ?? '')
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

          <div className="flex flex-wrap items-end gap-2">
            <label className="min-w-72 flex-1">
              <span className="mb-1 block text-xs text-muted">{t('Dépôt surveillé (Gitea, Forgejo ou GitHub)', 'Watched repository (Gitea, Forgejo or GitHub)')}</span>
              <input
                className="h-9 w-full rounded-md border border-line bg-bg-2 px-3 text-sm disabled:opacity-60"
                placeholder="https://gitea.exemple.org/proprietaire/RustNVR"
                value={repository}
                disabled={busy || readOnly !== null}
                onChange={(e) => setRepository(e.target.value)}
              />
            </label>
            <Button icon={<Save className="size-4" />} disabledReason={readOnly ?? (busy ? t('En cours…', 'In progress…') : !changed ? t('Aucun changement', 'No change') : null)} onClick={() => void run(() => setUpdates(repository.trim() || null))}>
              {t('Enregistrer', 'Save')}
            </Button>
            <Button icon={<RefreshCw className={`size-4 ${busy ? 'animate-spin' : ''}`} />} disabledReason={readOnly ?? (busy ? t('En cours…', 'In progress…') : !u.repository ? t('Indiquez d’abord un dépôt', 'First enter a repository') : null)} onClick={() => void run(checkUpdates)}>
              {t('Vérifier maintenant', 'Check now')}
            </Button>
          </div>
          {error && <p className="text-err">{error}</p>}
          <p className="text-xs text-muted">
            {t('RustNVR ne télécharge ni n’installe rien : il compare sa version à la dernière publication (« release ») du dépôt et indique comment mettre à jour. Laissez vide pour ne rien vérifier. Dépôt privé : jeton dans une variable d’environnement, ', 'RustNVR neither downloads nor installs anything: it compares its version with the latest release of the repository and tells how to update. Leave empty to check nothing. Private repository: token in an environment variable, ')}<code className="font-mono">[updates] token_env</code>{t(' de config.toml.', ' of config.toml.')}
          </p>
        </div>
      )}
    </Card>
  )
}
