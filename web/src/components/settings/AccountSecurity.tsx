// The signed-in account's own security: its two-step login and its API tokens.

import { Check, Copy, KeyRound, ShieldCheck, ShieldOff, Trash } from 'lucide-react'
import { useState } from 'react'
import { describeError } from '../../api/client'
import { createToken, deleteToken, disableTotp, enableTotp, getTokens, getTotp, setupTotp, type ApiToken, type TotpSetup } from '../../api/endpoints'
import { useAsync } from '../../hooks/useAsync'
import { formatDateTime } from '../../lib/format'
import { t } from '../../lib/i18n'
import { Button, IconButton } from '../ui/Button'
import { Badge } from '../ui/Card'
import { ConfirmDialog } from '../ui/Modal'
import { ErrorState, LoadingState } from '../ui/States'

const inputCls = 'mt-1 h-9 w-full rounded-md border border-line bg-bg-2 px-3 text-sm text-fg outline-none focus:border-accent'

/** A text shown once (recovery codes, a new token), with a copy button. */
function ShownOnce({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false)
  const copy = () => {
    void navigator.clipboard?.writeText(text).then(
      () => {
        setCopied(true)
        window.setTimeout(() => setCopied(false), 2000)
      },
      () => undefined,
    )
  }
  return (
    <div className="space-y-2 rounded-md border border-warn/40 bg-warn/10 p-3">
      <p className="text-xs text-warn">{label}</p>
      <pre className="overflow-x-auto rounded bg-bg-2 p-2 text-sm whitespace-pre-wrap select-all">{text}</pre>
      <Button size="sm" icon={copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />} disabledReason={navigator.clipboard ? null : t('Copie impossible ici (page non HTTPS) : sélectionnez le texte.', 'Copying is not possible here (page not HTTPS): select the text.')} onClick={copy}>
        {copied ? t('Copié', 'Copied') : t('Copier', 'Copy')}
      </Button>
    </div>
  )
}

/** Double authentification: turned on with a first code, off with the password. */
export function OwnTwoStep() {
  const status = useAsync(() => getTotp(), [])
  const [setup, setSetup] = useState<TotpSetup | null>(null)
  const [code, setCode] = useState('')
  const [password, setPassword] = useState('')
  const [recovery, setRecovery] = useState<string[] | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const run = async (action: () => Promise<void>) => {
    setBusy(true)
    setError(null)
    try {
      await action()
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }
  if (status.error) return <ErrorState error={status.error} onRetry={status.reload} compact />
  if (!status.data) return <LoadingState compact />
  const enabled = status.data.enabled
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-medium">{t('Double authentification', 'Two-step login')}</h3>
        {enabled ? <Badge tone="ok">{t('Active', 'On')}</Badge> : <Badge>{t('Inactive', 'Off')}</Badge>}
      </div>
      <p className="max-w-2xl text-xs text-muted">
        {t(
          "En plus du mot de passe, un code à 6 chiffres d'une application d'authentification (Google Authenticator, Aegis, 2FAS…). Les scripts et Home Assistant passent alors par un jeton d'API.",
          'Besides the password, a 6-digit code of an authenticator app (Google Authenticator, Aegis, 2FAS…). Scripts and Home Assistant then use an API token.',
        )}
      </p>
      {recovery && (
        <ShownOnce
          text={recovery.join('\n')}
          label={t(
            'Codes de secours, à garder en lieu sûr : chacun remplace une fois le code de l’application si vous perdez le téléphone. Ils ne seront plus affichés.',
            'Recovery codes, to keep somewhere safe: each one replaces the app code once if you lose the phone. They will not be shown again.',
          )}
        />
      )}
      {enabled ? (
        <div className="space-y-2">
          <p className="text-xs text-muted">{t(`Codes de secours restants : ${status.data.recovery_codes_left}`, `Recovery codes left: ${status.data.recovery_codes_left}`)}</p>
          <div className="flex max-w-lg flex-wrap items-end gap-2">
            <label className="min-w-48 flex-1 text-xs font-medium text-muted">
              {t('Mot de passe, pour la désactiver', 'Password, to turn it off')}
              <input className={inputCls} type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} />
            </label>
            <Button
              size="sm"
              icon={<ShieldOff className="size-3.5" />}
              disabledReason={!password ? t('Mot de passe requis.', 'Password required.') : busy ? t('Enregistrement…', 'Saving…') : null}
              onClick={() =>
                void run(async () => {
                  await disableTotp(password)
                  setPassword('')
                  setRecovery(null)
                  status.reload()
                })
              }
            >
              {t('Désactiver', 'Turn off')}
            </Button>
          </div>
        </div>
      ) : setup ? (
        <div className="flex flex-wrap items-start gap-4 rounded-md border border-line p-3">
          {/* The QR code comes from this server (an SVG it made). */}
          <div className="size-48 shrink-0 rounded bg-white p-1 [&_svg]:size-full" dangerouslySetInnerHTML={{ __html: setup.qr_svg }} />
          <div className="min-w-56 flex-1 space-y-2 text-sm">
            <p>{t("1. Scannez ce QR code avec l'application d'authentification.", '1. Scan this QR code with the authenticator app.')}</p>
            <p className="text-xs text-muted">
              {t('Ou tapez ce secret : ', 'Or type this secret: ')}
              <code className="break-all select-all">{setup.secret}</code>
            </p>
            <label className="block text-xs font-medium text-muted">
              {t('2. Code affiché par l’application', '2. Code shown by the app')}
              <input className={inputCls} value={code} inputMode="numeric" autoComplete="one-time-code" onChange={(e) => setCode(e.target.value)} />
            </label>
            <div className="flex gap-2">
              <Button
                size="sm"
                variant="primary"
                icon={<ShieldCheck className="size-3.5" />}
                disabledReason={code.replace(/\s/g, '').length !== 6 ? t('Le code a 6 chiffres.', 'The code has 6 digits.') : busy ? t('Vérification…', 'Checking…') : null}
                onClick={() =>
                  void run(async () => {
                    const r = await enableTotp(code)
                    setRecovery(r.recovery_codes)
                    setSetup(null)
                    setCode('')
                    status.reload()
                  })
                }
              >
                {t('Activer', 'Turn on')}
              </Button>
              <Button size="sm" onClick={() => setSetup(null)}>
                {t('Annuler', 'Cancel')}
              </Button>
            </div>
          </div>
        </div>
      ) : (
        <Button size="sm" icon={<ShieldCheck className="size-3.5" />} disabledReason={busy ? t('Préparation…', 'Preparing…') : null} onClick={() => void run(async () => setSetup(await setupTotp()))}>
          {t('Activer la double authentification', 'Turn the two-step login on')}
        </Button>
      )}
      {error && <p className="text-xs text-err">{error}</p>}
    </div>
  )
}

/** API tokens of the signed-in account: shown once, listed, revoked. */
export function OwnTokens() {
  const list = useAsync(() => getTokens(), [])
  const [name, setName] = useState('')
  const [readOnly, setReadOnly] = useState(true)
  const [created, setCreated] = useState<ApiToken | null>(null)
  const [toDelete, setToDelete] = useState<ApiToken | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const run = async (action: () => Promise<void>) => {
    setBusy(true)
    setError(null)
    try {
      await action()
      list.reload()
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }
  if (list.error) return <ErrorState error={list.error} onRetry={list.reload} compact />
  if (!list.data) return <LoadingState compact />
  return (
    <div className="space-y-3">
      <h3 className="text-sm font-medium">{t("Jetons d'API", 'API tokens')}</h3>
      <p className="max-w-2xl text-xs text-muted">
        {t(
          'Pour un script, une tablette ou Home Assistant, sans donner votre mot de passe : en-tête « Authorization: Bearer <jeton> », ou le jeton comme mot de passe (HTTP Basic, avec votre identifiant). Un jeton ne peut pas gérer les comptes ni les jetons.',
          'For a script, a tablet or Home Assistant, without giving your password: header « Authorization: Bearer <token> », or the token as the password (HTTP Basic, with your username). A token cannot manage accounts or tokens.',
        )}
      </p>
      {created?.token && <ShownOnce text={created.token} label={t(`Jeton « ${created.name} » : copiez-le maintenant, il ne sera plus affiché.`, `Token « ${created.name} »: copy it now, it will not be shown again.`)} />}
      {list.data.length > 0 && (
        <ul className="divide-y divide-line rounded-md border border-line">
          {list.data.map((tok) => (
            <li key={tok.id} className="flex flex-wrap items-center gap-3 px-3 py-2 text-sm">
              <span className="min-w-32 font-medium">{tok.name}</span>
              {tok.read_only ? <Badge>{t('Lecture seule', 'Read only')}</Badge> : <Badge tone="warn">{t('Complet', 'Full')}</Badge>}
              <span className="text-xs text-muted">
                {t('créé le ', 'created on ')}
                {formatDateTime(tok.created_ms)}
                {' · '}
                {tok.last_used_ms ? t(`utilisé le ${formatDateTime(tok.last_used_ms)}`, `used on ${formatDateTime(tok.last_used_ms)}`) : t('jamais utilisé', 'never used')}
              </span>
              <span className="ml-auto">
                <IconButton label={t('Révoquer', 'Revoke')} icon={<Trash className="size-4" />} onClick={() => setToDelete(tok)} />
              </span>
            </li>
          ))}
        </ul>
      )}
      <div className="flex max-w-2xl flex-wrap items-end gap-3 rounded-md border border-line p-3">
        <label className="min-w-48 flex-1 text-xs font-medium text-muted">
          {t('Nom (l’appareil ou le script)', 'Name (the device or script)')}
          <input className={inputCls} value={name} maxLength={60} onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="flex items-center gap-2 pb-2 text-sm">
          <input type="checkbox" checked={readOnly} onChange={(e) => setReadOnly(e.target.checked)} />
          {t('Lecture seule', 'Read only')}
        </label>
        <Button
          size="sm"
          variant="primary"
          icon={<KeyRound className="size-3.5" />}
          disabledReason={!name.trim() ? t('Donnez un nom au jeton.', 'Give the token a name.') : busy ? t('Création…', 'Creating…') : null}
          onClick={() =>
            void run(async () => {
              setCreated(await createToken(name.trim(), readOnly))
              setName('')
            })
          }
        >
          {t('Créer', 'Create')}
        </Button>
      </div>
      {error && <p className="text-xs text-err">{error}</p>}
      <ConfirmDialog
        open={toDelete !== null}
        title={t('Révoquer ce jeton ?', 'Revoke this token?')}
        danger
        confirmLabel={t('Révoquer', 'Revoke')}
        message={toDelete && <>{t('Ce qui utilise le jeton ', 'Whatever uses the token ')}<strong className="text-fg">{toDelete.name}</strong>{t(' ne pourra plus se connecter.', ' will no longer be able to sign in.')}</>}
        onCancel={() => setToDelete(null)}
        onConfirm={() => {
          if (toDelete) void run(async () => void (await deleteToken(toDelete.id))).then(() => setToDelete(null))
        }}
      />
    </div>
  )
}
