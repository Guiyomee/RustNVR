import { Radar, Search } from 'lucide-react'
import { useEffect, useState } from 'react'
import { describeError } from '../../api/client'
import { discoverStreams, scanNetwork } from '../../api/endpoints'
import { formatResolution } from '../../lib/format'
import type { DiscoveryScan, DiscoveryStreams, FoundDevice, FoundStream } from '../../types/api'
import { Button } from '../ui/Button'
import { Badge } from '../ui/Card'
import { Modal } from '../ui/Modal'
import type { CameraPreset } from './CameraForm'
import { t } from '../../lib/i18n'

const inputCls = 'h-9 w-full rounded-md border border-line bg-bg-2 px-3 text-sm outline-none focus:border-accent disabled:opacity-60'

function streamLabel(s: FoundStream): string {
  const size = s.width && s.height ? ` · ${formatResolution(s.width, s.height)}` : ''
  return `${s.label} · ${s.codec ?? t('codec inconnu', 'unknown codec')}${size}${s.recordable ? '' : t(' (non enregistrable)', ' (cannot be recorded)')}`
}

function deviceTitle(d: FoundDevice): string {
  return d.name ?? d.hardware ?? d.server ?? (d.onvif ? t('Caméra ONVIF', 'ONVIF camera') : t('Serveur RTSP', 'RTSP server'))
}

/** One device: its credentials, then its streams to choose from. */
function DevicePanel({ device, onChoose }: { device: FoundDevice; onChoose: (p: CameraPreset) => void }) {
  const [username, setUsername] = useState('admin')
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<DiscoveryStreams | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [main, setMain] = useState('')
  const [sub, setSub] = useState('')
  const [name, setName] = useState('')
  const [onvifPort, setOnvifPort] = useState('')

  const find = async () => {
    setBusy(true)
    setError(null)
    setResult(null)
    try {
      const r = await discoverStreams({ address: device.address, xaddr: device.xaddr, rtsp_port: device.rtsp_port, server: device.server, username: username.trim() || null, password: password || null })
      setResult(r)
      setOnvifPort(r.onvif_port ? String(r.onvif_port) : '')
      const recordable = r.streams.filter((s) => s.recordable)
      const first = recordable[0]
      const last = recordable[recordable.length - 1]
      setMain(first?.url ?? '')
      setSub(last && last !== first ? last.url : '')
      setName([r.manufacturer, r.model].filter(Boolean).join(' ') || device.name || t(`Caméra ${device.address}`, `Camera ${device.address}`))
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="space-y-3 rounded-md border border-line p-3">
      <p className="text-sm font-medium">
        {deviceTitle(device)} <span className="font-mono text-xs text-muted">{device.address}</span>
      </p>
      <div className="grid gap-2 sm:grid-cols-[1fr_1fr_auto] sm:items-end">
        <label className="text-xs text-muted">
          {t('Identifiant', 'Username')}
          <input className={`${inputCls} mt-1`} value={username} autoComplete="off" onChange={(e) => setUsername(e.target.value)} />
        </label>
        <label className="text-xs text-muted">
          {t('Mot de passe', 'Password')}
          <input className={`${inputCls} mt-1`} type="password" value={password} autoComplete="new-password" onChange={(e) => setPassword(e.target.value)} />
        </label>
        <Button icon={<Search className="size-4" />} disabledReason={busy ? t('Recherche des flux… (jusqu’à une minute sans ONVIF)', 'Looking for the streams… (up to a minute without ONVIF)') : null} onClick={() => void find()}>
          {busy ? t('Recherche…', 'Looking…') : t('Trouver les flux', 'Find the streams')}
        </Button>
      </div>
      <p className="text-xs text-muted">{t('Ceux de la caméra. Certaines marques demandent de créer un utilisateur ONVIF dans leur interface. Le mot de passe ne sert qu’à cette recherche, puis au formulaire d’ajout.', 'Those of the camera. Some makers require an ONVIF user created in their interface. The password only serves this search, then the add form.')}</p>
      {error && <p className="text-sm text-err">{error}</p>}
      {result && (
        <div className="space-y-3">
          {result.error ? (
            <p className="rounded-md border border-err/40 bg-err/10 px-3 py-2 text-sm text-err">{result.error}</p>
          ) : (
            <p className="text-xs text-ok">
              {result.onvif ? t('Flux lus par ONVIF', 'Streams read through ONVIF') : t('Flux trouvés aux adresses habituelles de : ', 'Streams found at the usual addresses of: ')}
              {result.onvif ? [result.manufacturer, result.model].filter(Boolean).map((v) => ` · ${v}`).join('') : result.manufacturer}
            </p>
          )}
          {result.streams.length > 0 && (
            <>
              <label className="block text-xs text-muted">
                {t('Nom de la caméra', 'Camera name')}
                <input className={`${inputCls} mt-1`} value={name} onChange={(e) => setName(e.target.value)} />
              </label>
              <fieldset className="space-y-1">
                <legend className="text-xs font-medium text-muted">{t('Flux principal (enregistré)', 'Main stream (recorded)')}</legend>
                {result.streams.map((s) => (
                  <label key={s.url} className={`flex items-start gap-2 text-sm ${s.recordable ? '' : 'opacity-50'}`}>
                    <input type="radio" name="main" className="mt-1 accent-accent" disabled={!s.recordable} checked={main === s.url} onChange={() => setMain(s.url)} />
                    <span>
                      {streamLabel(s)}
                      <span className="block break-all font-mono text-xs text-muted">{s.url}</span>
                    </span>
                  </label>
                ))}
              </fieldset>
              <fieldset className="space-y-1">
                <legend className="text-xs font-medium text-muted">{t('Sous-flux (direct et détection de mouvement, plus léger)', 'Sub stream (live view and motion detection, lighter)')}</legend>
                <label className="flex items-center gap-2 text-sm">
                  <input type="radio" name="sub" className="accent-accent" checked={sub === ''} onChange={() => setSub('')} />
                  {t('Aucun', 'None')}
                </label>
                {result.streams
                  .filter((s) => s.url !== main && s.recordable)
                  .map((s) => (
                    <label key={s.url} className="flex items-center gap-2 text-sm">
                      <input type="radio" name="sub" className="accent-accent" checked={sub === s.url} onChange={() => setSub(s.url)} />
                      {streamLabel(s)}
                    </label>
                  ))}
              </fieldset>
              <div className="flex justify-end">
                <Button variant="primary" disabledReason={!main ? t('Choisissez un flux principal H.264 ou H.265.', 'Choose an H.264 or H.265 main stream.') : !name.trim() ? t('Donnez un nom à la caméra.', 'Give the camera a name.') : null} onClick={() => onChoose({ name: name.trim(), url: main, subUrl: sub, username: username.trim(), password, onvif: onvifPort })}>
                  {t('Continuer vers l’ajout', 'Continue to adding it')}
                </Button>
              </div>
            </>
          )}
        </div>
      )}
    </div>
  )
}

/** Cameras → « Rechercher sur le réseau ». */
export function DiscoveryDialog({ open, onClose, onChoose }: { open: boolean; onClose: () => void; onChoose: (p: CameraPreset) => void }) {
  const [network, setNetwork] = useState('')
  const [busy, setBusy] = useState(false)
  const [scan, setScan] = useState<DiscoveryScan | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [selected, setSelected] = useState<FoundDevice | null>(null)

  // A new search each time: nothing kept (passwords included) once closed.
  useEffect(() => {
    if (!open) {
      setScan(null)
      setSelected(null)
      setError(null)
    }
  }, [open])

  const run = async () => {
    setBusy(true)
    setError(null)
    setSelected(null)
    try {
      const r = await scanNetwork(network.trim() || null)
      setScan(r)
      if (!network.trim() && r.network) setNetwork(r.network)
    } catch (e) {
      setError(describeError(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Modal open={open} wide title={t('Rechercher des caméras sur le réseau', 'Find cameras on the network')} onClose={onClose} footer={<Button onClick={onClose}>{t('Fermer', 'Close')}</Button>}>
      <div className="space-y-4">
        <p className="text-sm text-muted">{t('RustNVR cherche les caméras ONVIF (elles répondent d’elles-mêmes) et les serveurs RTSP (ports 554 et 8554) du réseau local. Rien n’est ajouté sans votre accord.', 'RustNVR looks for ONVIF cameras (they answer by themselves) and RTSP servers (ports 554 and 8554) on the local network. Nothing is added without your consent.')}</p>
        <div className="flex flex-wrap items-end gap-2">
          <label className="min-w-56 flex-1 text-xs text-muted">
            {t('Réseau', 'Network')}
            <input className={`${inputCls} mt-1`} value={network} placeholder={t('automatique (le /24 de RustNVR), ex. 192.168.1.0/24', 'automatic (RustNVR’s /24), e.g. 192.168.1.0/24')} onChange={(e) => setNetwork(e.target.value)} />
          </label>
          <Button variant="primary" icon={<Radar className="size-4" />} disabledReason={busy ? t('Recherche en cours… (≈ 5 s)', 'Searching… (≈ 5 s)') : null} onClick={() => void run()}>
            {busy ? t('Recherche…', 'Searching…') : t('Rechercher', 'Search')}
          </Button>
        </div>
        {error && <p className="text-sm text-err">{error}</p>}
        {scan && (
          <div className="space-y-2">
            <p className="text-xs text-muted">
              {t(`${scan.devices.length} appareil(s)`, `${scan.devices.length} device(s)`)}
              {scan.network ? t(` sur ${scan.network}`, ` on ${scan.network}`) : t(' (ONVIF seulement : réseau local inconnu)', ' (ONVIF only: local network unknown)')}
              {t(` en ${(scan.took_ms / 1000).toFixed(1)} s.`, ` in ${(scan.took_ms / 1000).toFixed(1)} s.`)}
            </p>
            {scan.devices.length === 0 ? (
              <p className="text-sm text-muted">{t('Aucun appareil trouvé. Vérifiez le réseau indiqué. Sous Docker, la recherche ONVIF demande le réseau de l’hôte (network_mode: host) ; sous WSL, seul le balayage RTSP fonctionne.', 'No device found. Check the network above. Under Docker, the ONVIF search needs the host network (network_mode: host); under WSL, only the RTSP scan works.')}</p>
            ) : (
              <ul className="divide-y divide-line rounded-md border border-line">
                {scan.devices.map((d) => (
                  <li key={d.address}>
                    <button type="button" className={`flex w-full flex-wrap items-center gap-2 px-3 py-2 text-left text-sm hover:bg-line/30 ${selected?.address === d.address ? 'bg-accent/10' : ''}`} onClick={() => setSelected(d)}>
                      <span className="w-28 font-mono text-xs">{d.address}</span>
                      <span className="min-w-0 flex-1 truncate">{deviceTitle(d)}</span>
                      {d.onvif && <Badge tone="info">ONVIF</Badge>}
                      {d.rtsp_port && <Badge>{`RTSP :${d.rtsp_port}`}</Badge>}
                      {d.camera && <Badge tone="ok">{t(`Déjà ajoutée : ${d.camera}`, `Already added: ${d.camera}`)}</Badge>}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
        {selected && <DevicePanel key={selected.address} device={selected} onChoose={onChoose} />}
      </div>
    </Modal>
  )
}
