"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { DaemonPortConflicts } from "@/components/app/daemon-port-conflicts";
import { bootstrapQueries, daemonMutations, systemMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";

type Status = "setup" | "off" | "running";

export const SettingsStatus = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const install = useMutation(systemMutations.install(queryClient));
  const startDaemon = useMutation(daemonMutations.start(queryClient));

  if (!data) return null;

  const appName = data.environment.appName;
  const isSetup =
    data.environment.cliHelperReady &&
    data.environment.cliAccessReady &&
    data.daemon.caInstalled;
  const status: Status = !isSetup ? "setup" : data.daemon.running ? "running" : "off";

  const copy = {
    setup: {
      title: `Set up ${appName}`,
      description: `A quick setup is needed before ${appName} can route your local projects and expose the \`localify\` terminal command.`,
    },
    off: {
      title: `${appName} is off`,
      description: `Turn ${appName} on to start routing your local projects.`,
    },
    running: {
      title: `${appName} is running`,
      description: "Your local projects are being routed.",
    },
  }[status];

  const errorMessage =
    status === "setup" && install.error instanceof Error
      ? install.error.message
      : status === "off" && startDaemon.error instanceof Error
        ? startDaemon.error.message
        : null;
  const showPortConflicts = Boolean(errorMessage?.includes("already in use"));

  return (
    <Card className="p-5">
      <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex min-w-0 items-start gap-3">
          <StatusDot status={status} />
          <div className="min-w-0 space-y-0.5">
            <p className="text-sm font-medium text-[--color-ink]">{copy.title}</p>
            <p className="text-xs text-[--color-muted]">{copy.description}</p>
          </div>
        </div>

        {status === "setup" ? (
          <Button
            variant="primary"
            loading={install.isPending}
            onClick={() => install.mutate()}
          >
            Set up
          </Button>
        ) : status === "off" ? (
          <Button
            variant="primary"
            loading={startDaemon.isPending}
            onClick={() => startDaemon.mutate({ verbose: false })}
          >
            Turn on
          </Button>
        ) : null}
      </div>

      {errorMessage ? (
        <p className="mt-3 text-xs text-[--color-danger]">{errorMessage}</p>
      ) : null}

      <div className="mt-3">
        <DaemonPortConflicts visible={showPortConflicts} />
      </div>
    </Card>
  );
};

const StatusDot = ({ status }: { status: Status }) => {
  return (
    <span
      aria-hidden
      className={cn(
        "mt-1 inline-flex size-2.5 shrink-0 rounded-full",
        status === "running" && "bg-emerald-500 shadow-[0_0_0_3px_rgb(16_185_129_/_0.15)]",
        status === "off" && "bg-[--color-muted]/60",
        status === "setup" && "bg-amber-500 shadow-[0_0_0_3px_rgb(245_158_11_/_0.15)]",
      )}
    />
  );
};
