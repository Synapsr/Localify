"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { motion } from "motion/react";
import { useState } from "react";
import { toast } from "sonner";

import { DaemonPortConflicts } from "@/components/app/daemon-port-conflicts";
import { CheckIcon, ServersIcon, TerminalIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { bootstrapQueries, daemonMutations, systemMutations } from "@/lib/queries";
import { getInstallWarningMessage } from "@/lib/util.install-result";
import { cn } from "@/lib/utils";

type Status = "setup" | "off" | "running";

async function copySetupError(errorMessage: string) {
  await navigator.clipboard.writeText(errorMessage);
  toast.success("Setup error copied");
}

function showSetupFailureToast(errorMessage: string) {
  toast.error("Setup failed", {
    description: errorMessage,
    action: {
      label: "Copy error",
      onClick: () => void copySetupError(errorMessage),
    },
  });
}

export const SettingsStatus = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const install = useMutation(systemMutations.install(queryClient));
  const startDaemon = useMutation(daemonMutations.start(queryClient));
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [setupWarning, setSetupWarning] = useState<string | null>(null);

  if (!data) return null;

  const appName = data.environment.appName;
  // Trust waits until someone asks for it. That is not a broken setup: only HTTPS in
  // browsers waits.
  const isSetup =
    !data.environment.systemSetupRequired &&
    data.environment.cliAccessReady &&
    (data.daemon.caInstalled || data.daemon.caTrustPending);
  const daemonRunning = data.daemon.running;
  const status: Status = !isSetup ? "setup" : daemonRunning ? "running" : "off";

  const copy = {
    setup: {
      title: `Set up ${appName}`,
      description: "A quick setup is needed before we can route your projects.",
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

  const installError = install.error instanceof Error ? install.error.message : null;
  const daemonError =
    startDaemon.error instanceof Error ? startDaemon.error.message : null;
  const setupError = installError ?? setupWarning;
  const portConflict = Boolean(
    (status === "setup" && setupError?.includes("already in use")) ||
      (status === "off" && daemonError?.includes("already in use")),
  );

  async function handleInstall() {
    setSetupWarning(null);

    try {
      const result = await install.mutateAsync();
      const warning = getInstallWarningMessage(result);

      if (warning) {
        setSetupWarning(warning);
        showSetupFailureToast(warning);
        return;
      }

      if (!daemonRunning) {
        await startDaemon.mutateAsync({ verbose: false });
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      showSetupFailureToast(message);
    }
  }

  const primaryAction =
    status === "setup" ? (
      <Button
        size="lg"
        variant="primary"
        loading={install.isPending}
        onClick={handleInstall}
      >
        Set up
      </Button>
    ) : status === "off" ? (
      <Button
        size="lg"
        variant="primary"
        loading={startDaemon.isPending}
        onClick={() => startDaemon.mutate({ verbose: false })}
      >
        Turn on
      </Button>
    ) : (
      <Button size="lg" variant="outline" onClick={() => setDetailsOpen(true)}>
        Details
      </Button>
    );

  return (
    <section className="space-y-2">
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ type: "spring", duration: 0.5, bounce: 0 }}
        className="relative isolate overflow-hidden rounded-2xl border border-border/40 bg-neutral-50/80 px-6 py-5 shadow-[0_1px_0_0_rgba(255,255,255,0.6)_inset] transition-colors duration-200 hover:border-border dark:bg-neutral-900/50 dark:shadow-[0_1px_0_0_rgba(255,255,255,0.04)_inset]"
      >
        <div className="flex items-center gap-5">
          <StatusIndicator status={status} />
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <h3 className="text-[17px] font-semibold tracking-tight text-foreground">
              {copy.title}
            </h3>
            <p className="max-w-md text-[13.5px] leading-normal text-pretty text-muted-foreground">
              {copy.description}
            </p>
          </div>
          {primaryAction}
        </div>
      </motion.div>

      {setupError || daemonError ? (
        <p className="px-1 text-xs text-[--color-danger] whitespace-pre-line">
          {setupError ?? daemonError}
        </p>
      ) : null}

      <DaemonPortConflicts visible={portConflict} />

      <SystemDetailsDialog open={detailsOpen} onOpenChange={setDetailsOpen} />
    </section>
  );
};

const StatusIndicator = ({ status }: { status: Status }) => (
  <span
    aria-hidden
    className={cn(
      "relative inline-flex size-10 shrink-0 items-center justify-center rounded-full",
      status === "running" && "bg-emerald-500/10",
      status === "off" && "bg-muted/60",
      status === "setup" && "bg-amber-500/10",
    )}
  >
    <span
      className={cn(
        "inline-flex size-3 rounded-full",
        status === "running" && "bg-emerald-500 shadow-[0_0_0_4px_rgb(16_185_129/0.2)]",
        status === "off" && "bg-muted-foreground/40",
        status === "setup" && "bg-amber-500 shadow-[0_0_0_4px_rgb(245_158_11/0.2)]",
      )}
    />
  </span>
);

const SystemDetailsDialog = ({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const startDaemon = useMutation(daemonMutations.start(queryClient));
  const setupCliAccess = useMutation(systemMutations.setupCliAccess(queryClient));
  const setupNodeTrust = useMutation(systemMutations.setupNodeTrust(queryClient));

  if (!data) return null;

  const cliError =
    setupCliAccess.error instanceof Error ? setupCliAccess.error.message : null;
  const nodeTrustError =
    setupNodeTrust.error instanceof Error ? setupNodeTrust.error.message : null;
  const daemonError =
    startDaemon.error instanceof Error ? startDaemon.error.message : null;
  const actionError = cliError ?? nodeTrustError ?? daemonError;

  const appName = data.environment.appName;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>System details</DialogTitle>
          <DialogDescription>
            The pieces {appName} needs to route your local projects.
          </DialogDescription>
        </DialogHeader>
        <DialogBody className="pb-4">
          <ul className="space-y-2">
            <DetailRow
              icon={<TerminalIcon />}
              title="Terminal command"
              description={`Use ${appName.toLowerCase()} from any shell or VS Code task.`}
              ready={data.environment.cliAccessReady}
              action={
                <Button
                  size="sm"
                  variant="outline"
                  loading={setupCliAccess.isPending}
                  onClick={() => setupCliAccess.mutate()}
                >
                  Install
                </Button>
              }
            />
            <DetailRow
              icon={<TerminalIcon />}
              title="Node.js certificate trust"
              description="New terminals trust .localify HTTPS automatically."
              ready={data.environment.nodeTrust.ready}
              action={
                <Button
                  size="sm"
                  variant="outline"
                  loading={setupNodeTrust.isPending}
                  onClick={() => setupNodeTrust.mutate()}
                >
                  Repair
                </Button>
              }
            />
            <DetailRow
              icon={<ServersIcon />}
              title="Background service"
              description="Routes local domains while the app is open."
              ready={data.daemon.running}
              action={
                <Button
                  size="sm"
                  variant="outline"
                  loading={startDaemon.isPending}
                  onClick={() => startDaemon.mutate({ verbose: false })}
                >
                  Start
                </Button>
              }
            />
          </ul>

          {actionError ? (
            <p className="mt-4 text-xs text-[--color-danger]">{actionError}</p>
          ) : null}
        </DialogBody>
        <DialogFooter showCloseButton />
      </DialogContent>
    </Dialog>
  );
};

const DetailRow = ({
  icon,
  title,
  description,
  ready,
  action,
}: {
  icon: React.ReactNode;
  title: string;
  description: string;
  ready: boolean;
  action: React.ReactNode;
}) => (
  <li className="flex items-center gap-3 rounded-xl border border-border/40 bg-neutral-50/60 px-3 py-3 dark:bg-neutral-800/40">
    <span
      aria-hidden
      className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-background text-foreground/80 ring-1 ring-border/60 [&>svg]:size-4"
    >
      {icon}
    </span>
    <div className="min-w-0 flex-1">
      <div className="truncate text-sm font-medium text-foreground">{title}</div>
      <div className="truncate text-xs text-muted-foreground">{description}</div>
    </div>
    {ready ? (
      <span className="inline-flex items-center gap-1 text-xs font-medium text-emerald-600 dark:text-emerald-500">
        <CheckIcon className="size-3.5" />
        Ready
      </span>
    ) : (
      action
    )}
  </li>
);
