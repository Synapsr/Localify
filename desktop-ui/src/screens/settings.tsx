import { useState } from "react";
import { Button } from "../components/ui/button";
import { Card } from "../components/ui/card";
import { Input, Label } from "../components/ui/input";
import {
  useBootstrap,
  useInstall,
  useUninstall,
  useUninstallPreview,
  useUpdateConfig,
} from "../lib/queries";

export function SettingsScreen() {
  const { data } = useBootstrap();
  const updateConfig = useUpdateConfig();
  const install = useInstall();
  const uninstall = useUninstall();
  const appName = data?.environment.appName ?? "Localify";

  const daemon = data?.config.daemon;
  const [httpPort, setHttpPort] = useState(daemon?.httpPort ?? 80);
  const [httpsPort, setHttpsPort] = useState(daemon?.httpsPort ?? 443);
  const [dnsPort, setDnsPort] = useState(daemon?.dnsPort ?? 1053);
  const [confirmingUninstall, setConfirmingUninstall] = useState(false);
  const [uninstallError, setUninstallError] = useState<string | null>(null);
  const uninstallPreview = useUninstallPreview(confirmingUninstall);

  if (!data || !daemon) return null;

  async function savePorts() {
    if (!daemon) return;
    await updateConfig.mutateAsync({
      daemon: {
        ...daemon,
        httpPort,
        httpsPort,
        dnsPort,
      },
    });
  }

  async function handleUninstall() {
    setUninstallError(null);
    try {
      await uninstall.mutateAsync();
      setConfirmingUninstall(false);
    } catch (err) {
      setUninstallError(err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <section className="space-y-8">
      <div className="space-y-1">
        <h1 className="text-xl font-semibold tracking-tight">Settings</h1>
        <p className="text-sm text-[--color-muted]">
          Daemon configuration and system integration.
        </p>
      </div>

      <Card className="p-5">
        <div className="space-y-5">
          <div className="space-y-1">
            <h2 className="text-sm font-semibold">Ports</h2>
            <p className="text-xs text-[--color-muted]">
              Changes take effect on the next daemon restart. In system mode,
              saves go through the installed helper instead of writing
              directly into system files.
            </p>
          </div>
          <div className="grid gap-4 sm:grid-cols-3">
            <div className="space-y-1.5">
              <Label htmlFor="http">HTTP</Label>
              <Input
                id="http"
                type="number"
                value={httpPort}
                onChange={(e) => setHttpPort(Number(e.target.value))}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="https">HTTPS</Label>
              <Input
                id="https"
                type="number"
                value={httpsPort}
                onChange={(e) => setHttpsPort(Number(e.target.value))}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="dns">DNS</Label>
              <Input
                id="dns"
                type="number"
                value={dnsPort}
                onChange={(e) => setDnsPort(Number(e.target.value))}
              />
            </div>
          </div>
          <div className="flex justify-end">
            <Button
              variant="primary"
              loading={updateConfig.isPending}
              onClick={savePorts}
            >
              Save
            </Button>
          </div>
        </div>
      </Card>

      <Card className="p-5">
        <div className="space-y-4">
          <div>
            <h2 className="text-sm font-semibold">System</h2>
            <p className="text-xs text-[--color-muted]">
              Certificate authority trust for this macOS account, DNS resolver, and desktop helper
              integration.
            </p>
          </div>
          <dl className="grid gap-3 text-sm sm:grid-cols-2">
            <Row
              label="CA trusted in this account"
              value={data.daemon.caInstalled ? "Yes" : "No"}
            />
            <Row label="LAN IP" value={data.daemon.lanIp} />
            <Row label="Config" value={data.environment.configPath} mono />
            <Row label="Log file" value={data.config.paths.logFile} mono />
          </dl>
          <div className="flex justify-end gap-2">
            <Button
              variant="secondary"
              loading={install.isPending}
              onClick={() => install.mutate()}
            >
              Reinstall
            </Button>
          </div>
        </div>
      </Card>

      <Card className="border-[--color-danger]/30 p-5">
        <div className="space-y-4">
          <div>
            <h2 className="text-sm font-semibold text-[--color-danger]">
              Danger zone
            </h2>
            <p className="text-xs text-[--color-muted]">
              Remove {appName}, its certificates, DNS resolver changes, and the
              privileged desktop helper.
            </p>
          </div>
          {confirmingUninstall ? (
            <div className="space-y-4 rounded-lg border border-[--color-danger]/20 bg-[--color-danger]/5 p-4">
              <div className="space-y-1">
                <p className="text-sm font-medium text-[--color-ink]">
                  Confirm full uninstall
                </p>
                <p className="text-xs text-[--color-muted]">
                  This removes certificates, DNS resolver changes, registered
                  domains, and the privileged desktop helper.
                </p>
              </div>

              {uninstallPreview.isPending ? (
                <p className="text-xs text-[--color-muted]">
                  Checking what will be removed…
                </p>
              ) : uninstallPreview.data ? (
                <dl className="grid gap-3 text-sm sm:grid-cols-2">
                  <Row
                    label="Registered domains"
                    value={String(uninstallPreview.data.domainCount)}
                  />
                  <Row
                    label="Data directory"
                    value={uninstallPreview.data.dataDir}
                    mono
                  />
                </dl>
              ) : null}

              {uninstallPreview.error && (
                <p className="text-xs text-[--color-danger]">
                  {uninstallPreview.error.message}
                </p>
              )}

              {uninstallError && (
                <p className="text-xs text-[--color-danger]">
                  {uninstallError}
                </p>
              )}

              <div className="flex justify-end gap-2">
                <Button
                  variant="ghost"
                  onClick={() => {
                    setConfirmingUninstall(false);
                    setUninstallError(null);
                  }}
                  disabled={uninstall.isPending}
                >
                  Cancel
                </Button>
                <Button
                  variant="danger"
                  loading={uninstall.isPending}
                  onClick={handleUninstall}
                >
                  Confirm uninstall
                </Button>
              </div>
            </div>
          ) : (
            <div className="flex justify-end">
              <Button
                variant="danger"
                onClick={() => {
                  setConfirmingUninstall(true);
                  setUninstallError(null);
                }}
              >
                Uninstall {appName}
              </Button>
            </div>
          )}
        </div>
      </Card>
    </section>
  );
}

function Row({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="min-w-0 space-y-0.5">
      <dt className="text-xs text-[--color-muted]">{label}</dt>
      <dd
        className={
          mono
            ? "truncate font-mono text-xs text-[--color-ink]"
            : "truncate text-sm text-[--color-ink]"
        }
      >
        {value}
      </dd>
    </div>
  );
}
