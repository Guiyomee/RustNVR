import { Save } from 'lucide-react'
import { useState } from 'react'
import { describeError } from '../../api/client'
import { useReadOnly } from '../../auth/AuthGate'
import { getMqtt, setMqtt } from '../../api/endpoints'
import { useCapability, useIsDemo } from '../../data/DataContext'
import { useAsync } from '../../hooks/useAsync'
import type { MqttInput, MqttView } from '../../types/api'
import { Button } from '../ui/Button'
import { Badge } from '../ui/Card'
import { ErrorState, LoadingState } from '../ui/States'
import { t } from '../../lib/i18n'

const inputCls = 'mt-1 h-9 w-full rounded-md border border-line bg-bg-2 px-3 text-sm text-fg outline-none focus:border-accent disabled:opacity-60'

interface Form {
  enabled: boolean
  host: string
  port: string
  tls: boolean
  caFile: string
  username: string
  /** Empty: keep the stored password. */
  password: string
  clearPassword: boolean
  client_id: string
  topic_prefix: string
  discovery: boolean
  discovery_prefix: string
}

function toForm(v: MqttView): Form {
  return {
    enabled: v.enabled,
    host: v.host,
    port: String(v.port),
    tls: v.tls,
    caFile: v.ca_file ?? '',
    username: v.username ?? '',
    password: '',
    clearPassword: false,
    client_id: v.client_id,
    topic_prefix: v.topic_prefix,
    discovery: v.discovery,
    discovery_prefix: v.discovery_prefix,
  }
}

const topicOk = (t: string) => t.length > 0 && !t.startsWith('/') && !t.endsWith('/') && !/[#+]/.test(t)

function Status({ v }: { v: MqttView }) {
  if (!v.enabled) return <Badge>{t('Désactivé', 'Off')}</Badge>
  if (v.connected) return <Badge tone="ok">{t('Connecté', 'Connected')}</Badge>
  return <Badge tone="err">{t('Non connecté', 'Not connected')}</Badge>
}

/** Paramètres → MQTT / Home Assistant. */
export function MqttSettings() {
  const demo = useIsDemo()
  const available = useCapability('mqtt') === true
  const viewerReason = useReadOnly()
  const view = useAsync(() => (demo || !available ? Promise.resolve(null) : getMqtt()), [demo, available])
  const [form, setForm] = useState<Form | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [saved, setSaved] = useState<MqttView | null>(null)

  if (demo) return <p className="px-5 py-4 text-sm text-muted">{t('Les réglages MQTT ne sont pas disponibles en mode démo.', 'MQTT settings are not available in demo mode.')}</p>
  if (!available) return <p className="px-5 py-4 text-sm text-muted">{t("Ce serveur ne permet pas de configurer MQTT depuis l'interface.", "This server does not allow configuring MQTT from the interface.")}</p>
  if (view.error) return <ErrorState error={view.error} onRetry={view.reload} compact />
  const v = saved ?? view.data
  if (!v) return <LoadingState compact />
  const f = form ?? toForm(v)
  const set = (patch: Partial<Form>) => {
    setForm({ ...f, ...patch })
    setError(null)
  }
  const port = Number(f.port)
  const problems = [
    f.enabled && !f.host.trim() && t("L'adresse du broker est obligatoire.", "The broker address is required."),
    (!Number.isInteger(port) || port < 1 || port > 65535) && t('Le port doit être compris entre 1 et 65535.', 'The port must be between 1 and 65535.'),
    !f.client_id.trim() && t("L'identifiant client est obligatoire.", "The client identifier is required."),
    !topicOk(f.topic_prefix.trim()) && t('Préfixe des topics : sans « # », « + » ni « / » au début ou à la fin.', 'Topic prefix: no « # », « + » nor « / » at the start or end.'),
    f.discovery && !topicOk(f.discovery_prefix.trim()) && t('Préfixe de découverte invalide.', 'Invalid discovery prefix.'),
    f.password && !f.username.trim() && t("Un mot de passe nécessite un nom d'utilisateur.", "A password needs a user name."),
    f.tls && f.caFile.trim() && !f.caFile.trim().startsWith('/') && t('Le certificat doit être un chemin absolu.', 'The certificate must be an absolute path.'),
  ].filter(Boolean) as string[]

  const save = async () => {
    setBusy(true)
    setError(null)
    try {
      const input: MqttInput = {
        enabled: f.enabled,
        host: f.host.trim(),
        port,
        tls: f.tls,
        ca_file: f.tls && f.caFile.trim() ? f.caFile.trim() : null,
        username: f.username.trim() || null,
        // null keeps the stored password; "" removes it.
        password: f.clearPassword ? '' : f.password || null,
        client_id: f.client_id.trim(),
        topic_prefix: f.topic_prefix.trim(),
        discovery: f.discovery,
        discovery_prefix: f.discovery_prefix.trim(),
      }
      const result = await setMqtt(input)
      setSaved(result)
      setForm(null)
      // The connection takes a moment: refresh the status shortly after.
      window.setTimeout(() => void getMqtt().then(setSaved, () => undefined), 2500)
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-4 px-5 py-4">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <Status v={v} />
        {v.enabled && !v.connected && v.error && <span className="text-xs text-err">{v.error}</span>}
        {!v.managed && <span className="text-xs text-muted">{t('Réglages actuels issus de config.toml : les enregistrer ici crée mqtt.ui.toml, qui les remplace.', 'Current settings come from config.toml: saving them here creates mqtt.ui.toml, which replaces them.')}</span>}
      </div>

      <label className="flex items-start gap-2 text-sm">
        <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={f.enabled} onChange={(e) => set({ enabled: e.target.checked })} />
        <span>
          {t('Se connecter à un broker MQTT', 'Connect to an MQTT broker')}
          <span className="block text-xs text-muted">{t('Par exemple le module complémentaire « Mosquitto broker » de Home Assistant.', 'For example the « Mosquitto broker » add-on of Home Assistant.')}</span>
        </span>
      </label>

      <div className="grid gap-4 sm:grid-cols-[minmax(0,1fr)_120px]">
        <label className="text-xs font-medium text-muted">
          {t('Adresse du broker', 'Broker address')}
          <input className={inputCls} value={f.host} placeholder="192.168.1.10" spellCheck={false} onChange={(e) => set({ host: e.target.value })} />
        </label>
        <label className="text-xs font-medium text-muted">
          {t('Port', 'Port')}
          <input className={inputCls} inputMode="numeric" value={f.port} onChange={(e) => set({ port: e.target.value })} />
        </label>
        <label className="flex items-start gap-2 text-sm sm:col-span-2">
          <input
            type="checkbox"
            className="mt-0.5 size-4 accent-accent"
            checked={f.tls}
            onChange={(e) => set({ tls: e.target.checked, port: f.port === (f.tls ? '8883' : '1883') ? (e.target.checked ? '8883' : '1883') : f.port })}
          />
          <span>
            {t('Connexion chiffrée (TLS)', 'Encrypted connection (TLS)')}
            <span className="block text-xs text-muted">{t('Port 8883 en général. Le certificat du broker est vérifié avec les autorités du système, ou le fichier ci-dessous.', 'Usually port 8883. The broker certificate is checked against the system authorities, or the file below.')}</span>
          </span>
        </label>
        {f.tls && (
          <label className="text-xs font-medium text-muted sm:col-span-2">
            {t("Certificat d'autorité (facultatif, broker auto-signé)", "Authority certificate (optional, self-signed broker)")}
            <input className={`${inputCls} font-mono`} value={f.caFile} placeholder="/home/user/mqtt-ca.crt" spellCheck={false} onChange={(e) => set({ caFile: e.target.value })} />
            <span className="mt-1 block font-normal">{t("Chemin d'un fichier PEM sur la machine de RustNVR.", "Path of a PEM file on the RustNVR machine.")}</span>
          </label>
        )}
        <label className="text-xs font-medium text-muted">
          {t('Utilisateur', 'User')}
          <input className={inputCls} value={f.username} autoComplete="off" onChange={(e) => set({ username: e.target.value })} />
        </label>
        <div />
        <label className="text-xs font-medium text-muted">
          {t('Mot de passe', 'Password')}
          <input
            className={inputCls}
            type="password"
            autoComplete="new-password"
            value={f.password}
            disabled={f.clearPassword}
            placeholder={v.has_password ? t('Inchangé (laisser vide pour conserver)', 'Unchanged (leave empty to keep)') : t('Aucun', 'None')}
            onChange={(e) => set({ password: e.target.value })}
          />
          {v.has_password && (
            <span className="mt-1 flex items-center gap-2 font-normal">
              <input type="checkbox" className="size-3.5 accent-accent" checked={f.clearPassword} onChange={(e) => set({ clearPassword: e.target.checked, password: '' })} />
              {t('Supprimer le mot de passe enregistré', 'Delete the saved password')}
            </span>
          )}
        </label>
      </div>

      <details className="text-sm">
        <summary className="cursor-pointer text-xs text-muted hover:text-fg">{t('Réglages avancés', 'Advanced settings')}</summary>
        <div className="mt-3 grid gap-4 sm:grid-cols-2">
          <label className="text-xs font-medium text-muted">
            {t('Identifiant client', 'Client identifier')}
            <input className={inputCls} value={f.client_id} onChange={(e) => set({ client_id: e.target.value })} />
          </label>
          <label className="text-xs font-medium text-muted">
            {t('Préfixe des topics', 'Topic prefix')}
            <input className={`${inputCls} font-mono`} value={f.topic_prefix} onChange={(e) => set({ topic_prefix: e.target.value })} />
          </label>
          <label className="flex items-start gap-2 text-sm sm:col-span-2">
            <input type="checkbox" className="mt-0.5 size-4 accent-accent" checked={f.discovery} onChange={(e) => set({ discovery: e.target.checked })} />
            <span>
              {t('Découverte automatique Home Assistant', 'Home Assistant discovery')}
              <span className="block text-xs text-muted">{t('Chaque caméra apparaît comme un appareil : mouvement, connexion, enregistrement, interrupteur de détection et image du dernier mouvement.', 'Each camera appears as a device: motion, connection, recording, detection switch and picture of the last motion.')}</span>
            </span>
          </label>
          <label className="text-xs font-medium text-muted">
            {t('Préfixe de découverte', 'Discovery prefix')}
            <input className={`${inputCls} font-mono`} value={f.discovery_prefix} disabled={!f.discovery} onChange={(e) => set({ discovery_prefix: e.target.value })} />
          </label>
        </div>
      </details>

      {error && <p className="rounded-md border border-err/40 bg-err/10 px-3 py-2 text-sm text-err">{error}</p>}
      <div className="flex flex-wrap items-center gap-3">
        <Button variant="primary" icon={<Save className="size-4" />} disabledReason={viewerReason ?? problems[0] ?? (form === null ? t('Aucun changement.', 'No change.') : busy ? t('Enregistrement…', 'Saving…') : null)} onClick={() => void save()}>
          {t('Enregistrer et se connecter', 'Save and connect')}
        </Button>
        <span className="text-xs text-muted">
          {t('Topics : ', 'Topics: ')}<code>{f.topic_prefix || 'rustnvr'}/&lt;caméra&gt;/motion</code>, <code>…/status</code>, <code>…/snapshot</code>, <code>…/motion_detection/set</code>{t(' (ON/OFF).', ' (ON/OFF).')}
        </span>
      </div>
    </div>
  )
}
