"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { bootstrapQueries, daemonMutations, systemMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";

export const SettingsServices = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const setupCliAccess = useMutation(systemMutations.setupCliAccess(queryClient));
  const startDaemon = useMutation(daemonMutations.start(queryClient));

  if (!data) return null;

  const appName = data.environment.appName;
  const cliError =
    setupCliAccess.error instanceof Error ? setupCliAccess.error.message : null;
  const daemonError =
    startDaemon.error instanceof Error ? startDaemon.error.message : null;

  return (
    <div className="grid gap-3 sm:grid-cols-2">
      <ServiceCard
        active={data.environment.cliAccessReady}
        title="Terminal command"
        description={`Install \`localify\` so your shell and VS Code can use ${appName} from any project.`}
        enabledLabel="Installed"
        actionLabel="Install"
        loading={setupCliAccess.isPending}
        onAction={() => setupCliAccess.mutate()}
        error={cliError}
      />
      <ServiceCard
        active={data.daemon.running}
        title="Background service"
        description="Routes your local projects while the app is open."
        enabledLabel="Running"
        actionLabel="Start"
        loading={startDaemon.isPending}
        onAction={() => startDaemon.mutate({ verbose: false })}
        error={daemonError}
      />
    </div>
  );
};

interface ServiceCardProps {
  active: boolean;
  title: string;
  description: string;
  enabledLabel: string;
  actionLabel: string;
  loading: boolean;
  onAction: () => void;
  error: string | null;
}

const ServiceCard = ({
  active,
  title,
  description,
  enabledLabel,
  actionLabel,
  loading,
  onAction,
  error,
}: ServiceCardProps) => {
  return (
    <Card className="p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-start gap-2.5">
          <span
            aria-hidden
            className={cn(
              "mt-1.5 inline-flex size-2 shrink-0 rounded-full",
              active ? "bg-emerald-500" : "bg-[--color-muted]/60",
            )}
          />
          <div className="min-w-0 space-y-0.5">
            <p className="text-sm font-medium text-[--color-ink]">{title}</p>
            <p className="text-xs text-[--color-muted]">{description}</p>
          </div>
        </div>
        <div className="shrink-0">
          {active ? (
            <span className="text-xs font-medium text-emerald-600">{enabledLabel}</span>
          ) : (
            <Button variant="outline" size="sm" loading={loading} onClick={onAction}>
              {actionLabel}
            </Button>
          )}
        </div>
      </div>
      {error ? <p className="mt-2 text-xs text-[--color-danger]">{error}</p> : null}
    </Card>
  );
};
