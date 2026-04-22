"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { DaemonPortConflicts } from "@/components/app/daemon-port-conflicts";
import { DownloadIcon, PowerIcon, ServersIcon, TerminalIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { useAppUpdate } from "@/hooks/use-app-update";
import { bootstrapQueries, daemonMutations, systemMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { SettingsGroup, SettingsRow, SettingsSection } from "./settings-list";

type Status = "setup" | "off" | "running";

export const SettingsStatus = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const install = useMutation(systemMutations.install(queryClient));
  const startDaemon = useMutation(daemonMutations.start(queryClient));
  const setupCliAccess = useMutation(systemMutations.setupCliAccess(queryClient));
  const setupNodeTrust = useMutation(systemMutations.setupNodeTrust(queryClient));
  const appUpdate = useAppUpdate();

  if (!data) return null;

  const appName = data.environment.appName;
  const isSetup =
    data.environment.cliHelperReady &&
    data.environment.cliAccessReady &&
    data.daemon.caInstalled;
  const status: Status = !isSetup ? "setup" : data.daemon.running ? "running" : "off";

  const statusCopy = {
    setup: { title: `Set up ${appName}`, value: "Setup needed" },
    off: { title: `${appName} is off`, value: "Off" },
    running: { title: `${appName} is running`, value: "Running" },
  }[status];

  const installError = install.error instanceof Error ? install.error.message : null;
  const daemonError =
    startDaemon.error instanceof Error ? startDaemon.error.message : null;
  const cliError =
    setupCliAccess.error instanceof Error ? setupCliAccess.error.message : null;
  const nodeTrustError =
    setupNodeTrust.error instanceof Error ? setupNodeTrust.error.message : null;
  const updateError = appUpdate.errorMessage;
  const portConflict = Boolean(
    (status === "setup" && installError?.includes("already in use")) ||
      (status === "off" && daemonError?.includes("already in use")),
  );
  const updateAction = appUpdate.availableUpdate ? (
    <Button
      variant="outline"
      size="sm"
      loading={appUpdate.isInstalling}
      disabled={appUpdate.isChecking}
      onClick={() => {
        void appUpdate.installUpdate();
      }}
    >
      Install
    </Button>
  ) : (
    <Button
      variant="outline"
      size="sm"
      loading={appUpdate.isChecking}
      disabled={appUpdate.isInstalling}
      onClick={() => {
        void appUpdate.checkForUpdates();
      }}
    >
      Check now
    </Button>
  );

  const statusAction =
    status === "setup" ? (
      <Button
        variant="primary"
        size="sm"
        loading={install.isPending}
        onClick={() => install.mutate()}
      >
        Set up
      </Button>
    ) : status === "off" ? (
      <Button
        variant="primary"
        size="sm"
        loading={startDaemon.isPending}
        onClick={() => startDaemon.mutate({ verbose: false })}
      >
        Turn on
      </Button>
    ) : null;

  return (
    <SettingsSection label="System" icon={<PowerIcon />}>
      <SettingsGroup>
        <SettingsRow
          icon={<StatusDot status={status} />}
          title={statusCopy.title}
          description={
            status === "running"
              ? "Your local projects are being routed."
              : status === "off"
                ? `Turn ${appName} on to start routing your local projects.`
                : "A quick setup is needed before we can route your projects."
          }
          value={!statusAction ? statusCopy.value : undefined}
          trailing={statusAction}
        />
        <SettingsRow
          icon={<TerminalIcon />}
          title="Terminal command"
          description={`Use ${appName.toLowerCase()} from any shell or VS Code task.`}
          value={data.environment.cliAccessReady ? "Installed" : undefined}
          trailing={
            data.environment.cliAccessReady ? undefined : (
              <Button
                variant="outline"
                size="sm"
                loading={setupCliAccess.isPending}
                onClick={() => setupCliAccess.mutate()}
              >
                Install
              </Button>
            )
          }
        />
        <SettingsRow
          icon={<TerminalIcon />}
          title="Node.js certificate trust"
          description="Lets new terminals and restarted apps trust .localify HTTPS automatically."
          value={data.environment.nodeTrust.ready ? "Enabled" : undefined}
          trailing={
            data.environment.nodeTrust.ready ? undefined : (
              <Button
                variant="outline"
                size="sm"
                loading={setupNodeTrust.isPending}
                onClick={() => setupNodeTrust.mutate()}
              >
                Repair
              </Button>
            )
          }
        />
        <SettingsRow
          icon={<ServersIcon />}
          title="Background service"
          description="Routes local domains while the app is open."
          value={data.daemon.running ? "Running" : undefined}
          trailing={
            data.daemon.running ? undefined : (
              <Button
                variant="outline"
                size="sm"
                loading={startDaemon.isPending}
                onClick={() => startDaemon.mutate({ verbose: false })}
              >
                Start
              </Button>
            )
          }
        />
        <SettingsRow
          icon={<DownloadIcon />}
          title="Desktop updates"
          description={
            appUpdate.availableUpdate
              ? `Version ${appUpdate.availableUpdate.version} is ready to download and install.`
              : "Checks for new Localify Desktop builds in the background and on demand."
          }
          value={
            appUpdate.availableUpdate
              ? `v${appUpdate.availableUpdate.version} available`
              : appUpdate.status === "upToDate"
                ? "Up to date"
                : appUpdate.status === "unconfigured"
                  ? "Unavailable"
                  : undefined
          }
          trailing={updateAction}
        />
      </SettingsGroup>

      {installError || daemonError || cliError || nodeTrustError || updateError ? (
        <p className="px-1 text-xs text-[--color-danger]">
          {installError ?? daemonError ?? cliError ?? nodeTrustError ?? updateError}
        </p>
      ) : null}

      <DaemonPortConflicts visible={portConflict} />
    </SettingsSection>
  );
};

const StatusDot = ({ status }: { status: Status }) => (
  <span
    aria-hidden
    className={cn(
      "inline-flex size-2.5 shrink-0 rounded-full",
      status === "running" && "bg-emerald-500 shadow-[0_0_0_3px_rgb(16_185_129/0.15)]",
      status === "off" && "bg-[--color-muted]/60",
      status === "setup" && "bg-amber-500 shadow-[0_0_0_3px_rgb(245_158_11/0.15)]",
    )}
  />
);
