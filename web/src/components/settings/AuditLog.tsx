import { RefreshCw } from 'lucide-react'
import { useCallback, useEffect, useState } from 'react'
import { describeError } from '../../api/client'
import { getAudit, type AuditEntry } from '../../api/endpoints'
import { useAuth } from '../../auth/AuthGate'
import { useIsDemo } from '../../data/DataContext'
import { formatDateTime } from '../../lib/format'
import { t } from '../../lib/i18n'
import { Button } from '../ui/Button'
import { Badge } from '../ui/Card'
import { LoadingState } from '../ui/States'

/** Lines asked for at a time. */
const PAGE = 100

/** What an action of the log is, in words: `login`, or `METHOD /path` of a request. */
const ACTIONS: [RegExp, (m: RegExpMatchArray) => string][] = [
  [/^login$/, () => t('Connexion', 'Sign in')],
  [/^login_failed$/, () => t('Connexion refusée', 'Sign-in refused')],
  [/^POST \/auth\/logout$/, () => t('Déconnexion', 'Sign out')],
  [/^POST \/auth\/setup$/, () => t('Création du premier administrateur', 'First administrator created')],
  [/^PUT \/auth\/password$/, () => t('Changement de son mot de passe', 'Own password changed')],
  [/^POST \/users$/, () => t("Création d'un compte", 'Account created')],
  [/^PUT \/users\/(.+)$/, (m) => t(`Modification du compte ${m[1]}`, `Account ${m[1]} changed`)],
  [/^DELETE \/users\/(.+)$/, (m) => t(`Suppression du compte ${m[1]}`, `Account ${m[1]} deleted`)],
  [/^POST \/cameras$/, () => t("Ajout d'une caméra", 'Camera added')],
  [/^PUT \/cameras\/([^/]+)$/, (m) => t(`Modification de la caméra ${m[1]}`, `Camera ${m[1]} changed`)],
  [/^DELETE \/cameras\/([^/]+)$/, (m) => t(`Suppression de la caméra ${m[1]}`, `Camera ${m[1]} deleted`)],
  [/^POST \/cameras\/([^/]+)\/export$/, (m) => t(`Export d'un extrait (${m[1]})`, `Clip exported (${m[1]})`)],
  [/^PUT \/cameras\/([^/]+)\/motion$/, (m) => t(`Réglages de détection (${m[1]})`, `Detection settings (${m[1]})`)],
  [/^POST \/cameras\/([^/]+)\/reboot$/, (m) => t(`Redémarrage de la caméra ${m[1]}`, `Camera ${m[1]} restarted`)],
  [/^PUT \/cameras\/([^/]+)\/pause$/, (m) => t(`Pause ou reprise de la caméra ${m[1]}`, `Camera ${m[1]} paused or resumed`)],
  [/^PUT \/cameras\/([^/]+)\/audio$/, (m) => t(`Son enregistré ou non (${m[1]})`, `Sound recording (${m[1]})`)],
  [/^POST \/cameras\/([^/]+)\/presence\/reset$/, (m) => t(`Compteur de présence remis à zéro (${m[1]})`, `Presence count reset (${m[1]})`)],
  [/^POST \/cameras\/([^/]+)\/ptz\/presets$/, (m) => t(`Position PTZ enregistrée (${m[1]})`, `PTZ preset saved (${m[1]})`)],
  [/^DELETE \/cameras\/([^/]+)\/ptz\/presets\/.+$/, (m) => t(`Position PTZ supprimée (${m[1]})`, `PTZ preset deleted (${m[1]})`)],
  [/^PUT \/settings$/, () => t('Réglages généraux modifiés', 'General settings changed')],
  [/^PUT \/mqtt$/, () => t('Réglages MQTT modifiés', 'MQTT settings changed')],
  [/^PUT \/ai$/, () => t("Réglages de l'IA modifiés", 'AI settings changed')],
  [/^POST \/ai\/models\/(.+)$/, (m) => t(`Téléchargement du modèle IA ${m[1]}`, `AI model ${m[1]} downloaded`)],
  [/^POST \/ai\/replay$/, () => t("Rejeu de l'IA sur des images gardées", 'AI replayed on kept pictures')],
  [/^DELETE \/ai\/captures\/([^/]+)\/(.+)$/, (m) => t(`Images gardées supprimées (${m[1]}, ${m[2]})`, `Kept pictures deleted (${m[1]}, ${m[2]})`)],
  [/^PUT \/webhooks$/, () => t('Webhooks modifiés', 'Webhooks changed')],
  [/^PUT \/updates$/, () => t('Recherche de mises à jour activée ou non', 'Update check turned on or off')],
  [/^PUT \/detection\/pause$/, () => t('Détection mise en pause ou reprise', 'Detection paused or resumed')],
  [/^POST \/storage\/destinations$/, () => t("Ajout d'un point de stockage", 'Storage destination added')],
  [/^PUT \/storage\/destinations\/(.+)$/, (m) => t(`Modification du point de stockage ${m[1]}`, `Storage destination ${m[1]} changed`)],
  [/^DELETE \/storage\/destinations\/(.+)$/, (m) => t(`Suppression du point de stockage ${m[1]}`, `Storage destination ${m[1]} deleted`)],
  [/^POST \/backup$/, () => t('Sauvegarde de la configuration téléchargée', 'Configuration backup downloaded')],
  [/^POST \/backup\/restore$/, () => t('Restauration de la configuration', 'Configuration restored')],
  [/^POST \/protected$/, () => t('Enregistrements protégés', 'Recordings protected')],
  [/^DELETE \/protected\/.+$/, () => t('Protection retirée', 'Protection removed')],
]

function describe(action: string): string {
  for (const [pattern, text] of ACTIONS) {
    const m = action.match(pattern)
    if (m) return text(m)
  }
  return action
}

function Result({ entry }: { entry: AuditEntry }) {
  if (entry.status < 400) return <Badge tone="ok">{t('Fait', 'Done')}</Badge>
  if (entry.status === 401 || entry.status === 403) return <Badge tone="err">{t('Refusé', 'Refused')}</Badge>
  return <Badge tone="warn">{t(`Échec (${entry.status})`, `Failed (${entry.status})`)}</Badge>
}

/** Paramètres → Activité des comptes: who signed in and who changed what. */
export function AuditLog() {
  const demo = useIsDemo()
  const { user } = useAuth()
  const [entries, setEntries] = useState<AuditEntry[] | null>(null)
  const [more, setMore] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const allowed = !demo && user?.role !== 'viewer'

  const load = useCallback(async (after: AuditEntry[] | null) => {
    setBusy(true)
    setError(null)
    try {
      const last = after?.[after.length - 1]
      const page = await getAudit({ before: last?.id, limit: PAGE })
      setEntries([...(after ?? []), ...page])
      setMore(page.length === PAGE)
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }, [])
  useEffect(() => {
    if (allowed) void load(null)
  }, [allowed, load])

  if (demo) return <p className="px-5 py-4 text-sm text-muted">{t('Non disponible en mode démo.', 'Not available in demo mode.')}</p>
  if (!allowed) return <p className="px-5 py-4 text-sm text-muted">{t("Le journal d'activité est réservé aux administrateurs.", 'The activity log is for administrators only.')}</p>
  return (
    <div className="space-y-3 px-5 py-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <p className="text-xs text-muted">
          {t(
            'Connexions (refusées aussi) et tout ce qui modifie quelque chose : réglages, caméras, comptes, exports. Les consultations ne sont pas notées. Gardé 180 jours.',
            'Sign-ins (refused ones too) and everything that changes something: settings, cameras, accounts, exports. Views are not recorded. Kept 180 days.',
          )}
        </p>
        <Button size="sm" icon={<RefreshCw className="size-3.5" />} disabledReason={busy ? t('Chargement…', 'Loading…') : null} onClick={() => void load(null)}>
          {t('Actualiser', 'Refresh')}
        </Button>
      </div>
      {error && <p className="text-sm text-err">{error}</p>}
      {!entries ? (
        !error && <LoadingState compact />
      ) : entries.length === 0 ? (
        <p className="text-sm text-muted">{t('Rien pour le moment.', 'Nothing yet.')}</p>
      ) : (
        <div className="overflow-x-auto rounded-md border border-line">
          <table className="w-full text-left text-sm">
            <thead className="text-xs text-muted">
              <tr className="border-b border-line">
                <th className="px-3 py-2 font-medium">{t('Date', 'Date')}</th>
                <th className="px-3 py-2 font-medium">{t('Compte', 'Account')}</th>
                <th className="px-3 py-2 font-medium">{t('Action', 'Action')}</th>
                <th className="px-3 py-2 font-medium">{t('Résultat', 'Result')}</th>
                <th className="px-3 py-2 font-medium">{t('Adresse', 'Address')}</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {entries.map((e) => (
                <tr key={e.id}>
                  <td className="whitespace-nowrap px-3 py-2 text-muted">{formatDateTime(e.at_ms)}</td>
                  <td className="px-3 py-2 font-medium">{e.username ?? '—'}</td>
                  <td className="px-3 py-2" title={e.action}>{describe(e.action)}</td>
                  <td className="px-3 py-2"><Result entry={e} /></td>
                  <td className="whitespace-nowrap px-3 py-2 text-muted"><code>{e.client || '—'}</code></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {entries && more && (
        <Button size="sm" disabledReason={busy ? t('Chargement…', 'Loading…') : null} onClick={() => void load(entries)}>
          {t('Plus anciens', 'Older')}
        </Button>
      )}
    </div>
  )
}
