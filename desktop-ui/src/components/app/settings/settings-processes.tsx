"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  bootstrapQueries,
  daemonMutations,
  systemMutations,
} from "@/lib/queries";

export const SettingsProcesses = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const startDaemon = useMutation(daemonMutations.start(queryClient));
  const setupCliAccess = useMutation(systemMutations.setupCliAccess(queryClient));

  if (!data) return null;

  const daemonError = startDaemon.error;
  const daemonErrorMessage = daemonError instanceof Error ? daemonError.message : null;
  const cliAccessError = setupCliAccess.error;
  const cliAccessErrorMessage =
    cliAccessError instanceof Error ? cliAccessError.message : null;

  return (
    <Card className="p-5">
      <div className="space-y-4">
        <div>
          <h2 className="text-sm font-semibold">Processes</h2>
          <p className="text-xs text-[--color-muted]">
            Check whether the local daemon and the privileged CLI helper are ready, and enable
            them if needed.
          </p>
        </div>

        <div className="grid gap-3">
          <div className="flex flex-col gap-3 rounded-lg border border-[--color-border] p-4 sm:flex-row sm:items-center sm:justify-between">
            <div className="min-w-0 space-y-1">
              <div className="flex items-center gap-2">
                <p className="text-sm font-medium text-[--color-ink]">Localify daemon</p>
                <Badge tone={data.daemon.running ? "accent" : "muted"}>
                  {data.daemon.running ? "Running" : "Stopped"}
                </Badge>
              </div>
              <p className="text-xs text-[--color-muted]">
                {data.daemon.running && data.daemon.pid
                  ? `PID ${data.daemon.pid} listening on ${data.daemon.httpPort}/${data.daemon.httpsPort}/${data.daemon.dnsPort}.`
                  : "The proxy, HTTPS, and DNS services are currently not running."}
              </p>
            </div>
            <Button
              variant={data.daemon.running ? "secondary" : "primary"}
              loading={startDaemon.isPending}
              disabled={data.daemon.running}
              onClick={() => startDaemon.mutate({ verbose: false })}
            >
              {data.daemon.running ? "Daemon active" : "Enable daemon"}
            </Button>
          </div>

          <div className="flex flex-col gap-3 rounded-lg border border-[--color-border] p-4 sm:flex-row sm:items-center sm:justify-between">
            <div className="min-w-0 space-y-1">
              <div className="flex items-center gap-2">
                <p className="text-sm font-medium text-[--color-ink]">CLI helper</p>
                <Badge tone={data.environment.cliHelperReady ? "accent" : "muted"}>
                  {data.environment.cliHelperReady ? "Enabled" : "Disabled"}
                </Badge>
              </div>
              <p className="text-xs text-[--color-muted]">
                Lets desktop actions write the system config through the installed helper.
              </p>
            </div>
            <Button
              variant={data.environment.cliHelperReady ? "secondary" : "primary"}
              loading={setupCliAccess.isPending}
              disabled={data.environment.cliHelperReady}
              onClick={() => setupCliAccess.mutate()}
            >
              {data.environment.cliHelperReady ? "CLI helper ready" : "Enable CLI helper"}
            </Button>
          </div>
        </div>

        {daemonErrorMessage ? (
          <p className="text-xs text-[--color-danger]">{daemonErrorMessage}</p>
        ) : null}

        {cliAccessErrorMessage ? (
          <p className="text-xs text-[--color-danger]">{cliAccessErrorMessage}</p>
        ) : null}
      </div>
    </Card>
  );
};
