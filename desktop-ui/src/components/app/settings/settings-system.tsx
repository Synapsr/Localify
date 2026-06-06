"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { bootstrapQueries, daemonMutations, systemMutations } from "@/lib/queries";
import { getInstallWarningMessage } from "@/lib/util.install-result";

import { SettingsRow } from "./settings-row";

export const SettingsSystem = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const install = useMutation(systemMutations.install(queryClient));
  const startDaemon = useMutation(daemonMutations.start(queryClient));

  if (!data) return null;

  const daemonRunning = data.daemon.running;
  const dnsResolution = data.daemon.dnsResolution;
  const dnsResolutionValue =
    daemonRunning && dnsResolution.checked
      ? dnsResolution.healthy
        ? "Healthy"
        : "Blocked"
      : "Not checked";

  async function handleReinstall() {
    try {
      const result = await install.mutateAsync();
      const warning = getInstallWarningMessage(result);

      if (warning) {
        toast.error("Setup needs attention", { description: warning });
        return;
      }

      if (!daemonRunning) {
        await startDaemon.mutateAsync({ verbose: false });
      }

      toast.success("System setup refreshed");
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      toast.error("Setup failed", { description: message });
    }
  }

  return (
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
          <SettingsRow
            label="CA trusted in this account"
            value={data.daemon.caInstalled ? "Yes" : "No"}
          />
          <SettingsRow
            label="CLI helper ready"
            value={data.environment.cliHelperReady ? "Yes" : "No"}
          />
          <SettingsRow
            label="CLI command installed"
            value={data.environment.cliAccessReady ? "Yes" : "No"}
          />
          <SettingsRow label="DNS resolution" value={dnsResolutionValue} />
          <SettingsRow label="LAN IP" value={data.daemon.lanIp} />
          <SettingsRow label="Config" value={data.environment.configPath} mono />
          <SettingsRow label="CLI command" value={data.environment.cliCommandPath} mono />
          <SettingsRow label="Log file" value={data.config.paths.logFile} mono />
        </dl>
        <div className="flex justify-end">
          <Button
            variant="secondary"
            loading={install.isPending || startDaemon.isPending}
            onClick={handleReinstall}
          >
            Reinstall
          </Button>
        </div>
      </div>
    </Card>
  );
};
