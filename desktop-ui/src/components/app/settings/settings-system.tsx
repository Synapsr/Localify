"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { bootstrapQueries, systemMutations } from "@/lib/queries";

import { SettingsRow } from "./settings-row";

export const SettingsSystem = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const install = useMutation(systemMutations.install(queryClient));

  if (!data) return null;

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
          <SettingsRow label="LAN IP" value={data.daemon.lanIp} />
          <SettingsRow label="Config" value={data.environment.configPath} mono />
          <SettingsRow label="CLI command" value={data.environment.cliCommandPath} mono />
          <SettingsRow label="Log file" value={data.config.paths.logFile} mono />
        </dl>
        <div className="flex justify-end">
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
  );
};
