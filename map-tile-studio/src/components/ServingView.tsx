import { useEffect, useState } from 'react';
import { Check, Copy, Globe, Play, Power, RefreshCw, Server, Square, Trash2 } from 'lucide-react';
import { serviceInstall, serviceSetRunning, serviceStatus, serviceUninstall } from '@/lib/api';
import type { ServiceInfo } from '@/lib/types';
import { cn } from '@/lib/utils';
import { Badge, Button, Field, TextInput } from '@/components/ui';

export function ServingView() {
  const [info, setInfo] = useState<ServiceInfo | null>(null);
  const [port, setPort] = useState('7765');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const refresh = async () => {
    try {
      const i = await serviceStatus();
      setInfo(i);
      setPort((p) => (p === '7765' || !p ? String(i.port) : p));
    } catch {
      /* ignore polling errors */
    }
  };
  useEffect(() => {
    refresh();
    const t = setInterval(refresh, 4000);
    return () => clearInterval(t);
  }, []);

  const installed = !!info && !['not installed', 'unknown'].includes(info.status);
  const running = info?.status === 'running';

  const act = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    setError('');
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
      await refresh();
    }
  };

  const template = `${info?.lan_url ?? `http://<this-pc>:${port}`}/{source}/{z}/{x}/{y}`;

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto bg-canvas">
      <div className="mx-auto w-full max-w-[760px] px-8 py-8">
        <div className="flex items-center gap-2.5">
          <div className="flex size-9 items-center justify-center rounded-xl bg-brand-tint">
            <Server className="size-[18px] text-brand" />
          </div>
          <div>
            <div className="font-semibold text-[18px] tracking-[-0.02em] text-ink">
              Publish over your network
            </div>
            <div className="text-[12.5px] text-muted">
              Serve your tile maps to other machines on the LAN — even with this app closed, and
              across restarts.
            </div>
          </div>
          <span className="flex-1" />
          <Button onClick={refresh} size="sm" variant="ghost">
            <RefreshCw className="size-3.5" />
          </Button>
        </div>

        {/* status card */}
        <div className="mt-6 rounded-[14px] border border-line bg-white p-5 shadow-[0_1px_2px_rgba(16,24,40,.04)]">
          <div className="flex items-center gap-2.5">
            <span
              className={cn(
                'size-2.5 rounded-full',
                running ? 'bg-ok mts-pulse' : installed ? 'bg-warn' : 'bg-[#cdd5e0]',
              )}
            />
            <span className="font-semibold text-[14px] text-ink">
              {!info ? 'Checking…' : running ? 'Service running' : installed ? 'Service stopped' : 'Not installed'}
            </span>
            <Badge tone={running ? 'ok' : installed ? 'warn' : 'neutral'}>
              {info?.status ?? '…'}
            </Badge>
          </div>

          {/* URL */}
          <div className="mt-4">
            <div className="mb-1.5 font-medium text-[11px] uppercase tracking-[0.04em] text-faint">
              Tile URL (XYZ)
            </div>
            <CopyField text={template} muted={!running} />
            <p className="mt-2 text-[11.5px] leading-relaxed text-muted">
              Replace <code className="font-mono text-[11px]">{'{source}'}</code> with a map name
              from the catalog. Paste into QGIS (XYZ Tiles), Leaflet/MapLibre, or a browser. Works
              while the service is <b>running</b>.
            </p>
          </div>

          {/* controls */}
          <div className="mt-5 border-line-soft border-t pt-5">
            {!installed ? (
              <div className="flex items-end gap-3">
                <div className="w-28">
                  <Field label="Port">
                    <TextInput
                      mono
                      onChange={(e) => setPort(e.target.value.replace(/[^0-9]/g, '').slice(0, 5))}
                      placeholder="7765"
                      value={port}
                    />
                  </Field>
                </div>
                <Button
                  busy={busy}
                  onClick={() => act(() => serviceInstall(Number(port) || 7765))}
                  variant="primary"
                >
                  <Power className="size-4" /> Install &amp; start service
                </Button>
                <span className="flex-1" />
                <span className="pb-2.5 text-[11px] text-faint">A Windows admin prompt will appear.</span>
              </div>
            ) : (
              <div className="flex items-center gap-2.5">
                {running ? (
                  <Button busy={busy} onClick={() => act(() => serviceSetRunning(false))} variant="secondary">
                    <Square className="size-3.5" /> Stop
                  </Button>
                ) : (
                  <Button busy={busy} onClick={() => act(() => serviceSetRunning(true))} variant="primary">
                    <Play className="size-[13px] fill-current" /> Start
                  </Button>
                )}
                <Button busy={busy} onClick={() => act(serviceUninstall)} variant="ghost">
                  <Trash2 className="size-3.5 text-danger" /> Uninstall
                </Button>
                <span className="flex-1" />
                <span className="font-mono text-[11px] text-faint">port {info?.port}</span>
              </div>
            )}
          </div>

          {error && (
            <div className="mt-4 rounded-lg bg-danger-tint px-3 py-2 text-[12px] leading-snug text-[#b42318]">
              {error}
            </div>
          )}
        </div>

        {/* notes */}
        <div className="mt-5 grid grid-cols-2 gap-3">
          <InfoCard icon={<Globe className="size-4 text-brand" />} title="LAN access">
            Other devices reach it at the address above. Installing opens the firewall port; for
            internet access put a reverse proxy (HTTPS) in front.
          </InfoCard>
          <InfoCard icon={<Server className="size-4 text-brand" />} title="Serving from">
            <span className="break-all font-mono text-[11px] text-muted">
              {info?.maps_dir ?? '—'}
            </span>
            <div className="mt-1">Every map you generate appears here automatically.</div>
          </InfoCard>
        </div>
      </div>
    </div>
  );
}

function CopyField({ text, muted }: { text: string; muted?: boolean }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      className={cn(
        'flex w-full items-center gap-2.5 rounded-[10px] border border-line bg-[#f7f8fa] px-3 py-2.5 text-left transition-colors hover:border-[#cdd5e0] hover:bg-[#f1f2f5]',
        muted && 'opacity-70',
      )}
      onClick={() => {
        navigator.clipboard?.writeText(text).catch(() => {});
        setCopied(true);
        setTimeout(() => setCopied(false), 1400);
      }}
      title="Copy"
      type="button"
    >
      <code className="min-w-0 flex-1 truncate font-mono text-[12.5px] text-ink">{text}</code>
      {copied ? (
        <Check className="size-4 flex-none text-ok" />
      ) : (
        <Copy className="size-4 flex-none text-faint" />
      )}
    </button>
  );
}

function InfoCard({
  icon,
  title,
  children,
}: {
  icon: React.ReactNode;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="rounded-[12px] border border-line bg-white p-3.5">
      <div className="mb-1.5 flex items-center gap-1.5">
        {icon}
        <span className="font-semibold text-[12.5px] text-ink">{title}</span>
      </div>
      <div className="text-[11.5px] leading-relaxed text-muted">{children}</div>
    </div>
  );
}
