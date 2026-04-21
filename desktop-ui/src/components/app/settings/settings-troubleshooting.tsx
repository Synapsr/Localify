"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import {
  DangerTriangleIcon,
  InfoCircleIcon,
  LockIcon,
  PowerIcon,
  TerminalIcon,
} from "@/components/icons";
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
import { cn } from "@/lib/utils";

import {
  buildTroubleshootingIssues,
  type TroubleshootingAction,
} from "./settings-troubleshooting.data";
import { SettingsGroup, SettingsRow, SettingsSection } from "./settings-list";

export const SettingsTroubleshooting = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const install = useMutation(systemMutations.install(queryClient));
  const setupCliAccess = useMutation(systemMutations.setupCliAccess(queryClient));
  const setupNodeTrust = useMutation(systemMutations.setupNodeTrust(queryClient));
  const startDaemon = useMutation(daemonMutations.start(queryClient));
  const [open, setOpen] = useState(false);

  if (!data) return null;

  const issues = buildTroubleshootingIssues(data);

  const handleAction = (action: TroubleshootingAction) => {
    if (action === "install") {
      install.mutate();
      return;
    }

    if (action === "setupCliAccess") {
      setupCliAccess.mutate();
      return;
    }

    if (action === "setupNodeTrust") {
      setupNodeTrust.mutate();
      return;
    }

    startDaemon.mutate({ verbose: false });
  };

  const getActionState = (action: TroubleshootingAction) => {
    if (action === "install") {
      return {
        loading: install.isPending,
        error: install.error instanceof Error ? install.error.message : null,
      };
    }

    if (action === "setupCliAccess") {
      return {
        loading: setupCliAccess.isPending,
        error:
          setupCliAccess.error instanceof Error
            ? setupCliAccess.error.message
            : null,
      };
    }

    if (action === "setupNodeTrust") {
      return {
        loading: setupNodeTrust.isPending,
        error:
          setupNodeTrust.error instanceof Error
            ? setupNodeTrust.error.message
            : null,
      };
    }

    return {
      loading: startDaemon.isPending,
      error: startDaemon.error instanceof Error ? startDaemon.error.message : null,
    };
  };

  return (
    <>
      <SettingsSection label="Troubleshooting" icon={<InfoCircleIcon />}>
        <SettingsGroup>
          <SettingsRow
            icon={<InfoCircleIcon />}
            title="Open troubleshooting guide"
            description="Known Localify issues, how to recognize them, and what to try next."
            onActivate={() => setOpen(true)}
          />
        </SettingsGroup>
      </SettingsSection>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-h-[calc(100vh-2rem)] overflow-y-auto overscroll-none sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>Troubleshooting</DialogTitle>
            <DialogDescription>
              Quick diagnostics for local HTTPS, terminal setup, background routing, and Node.js certificate trust.
            </DialogDescription>
          </DialogHeader>
          <DialogBody className="space-y-3 pb-6">
            {issues.map((issue) => {
              const action = issue.action;
              const actionState = issue.action
                ? getActionState(issue.action.id)
                : null;

              return (
                <article
                  key={issue.id}
                  className="space-y-3 rounded-xl border border-[--color-border]/70 bg-[--color-surface]/70 p-4"
                >
                  <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                    <div className="space-y-2">
                      <div className="flex items-center gap-2">
                        <span
                          className={cn(
                            "text-[--color-muted] [&>svg]:size-4",
                            issue.id === "port-conflicts" && "text-[--color-danger]",
                          )}
                        >
                          {issue.id === "ca-trust" ? (
                            <LockIcon />
                          ) : issue.id === "cli-access" || issue.id === "node-trust" ? (
                            <TerminalIcon />
                          ) : issue.id === "daemon" ? (
                            <PowerIcon />
                          ) : issue.id === "port-conflicts" ? (
                            <DangerTriangleIcon />
                          ) : (
                            <InfoCircleIcon />
                          )}
                        </span>
                        <h3 className="text-sm font-semibold text-[--color-ink]">
                          {issue.title}
                        </h3>
                        <span
                          className={cn(
                            "rounded-full px-2 py-0.5 text-[10px] font-medium uppercase tracking-wide",
                            issue.tone === "success" &&
                              "bg-emerald-500/12 text-emerald-700 dark:text-emerald-300",
                            issue.tone === "warning" &&
                              "bg-amber-500/12 text-amber-700 dark:text-amber-300",
                            issue.tone === "info" &&
                              "bg-[--color-border]/60 text-[--color-muted]",
                          )}
                        >
                          {issue.tone === "success"
                            ? "Looks good"
                            : issue.tone === "warning"
                              ? "Check this"
                              : "Heads up"}
                        </span>
                      </div>
                      <p className="text-sm text-[--color-muted]">{issue.summary}</p>
                    </div>

                    {action ? (
                      <Button
                        variant={issue.tone === "warning" ? "primary" : "outline"}
                        size="sm"
                        loading={actionState?.loading}
                        onClick={() => handleAction(action.id)}
                      >
                        {action.label}
                      </Button>
                    ) : null}
                  </div>

                  {issue.symptoms.length > 0 ? (
                    <div className="space-y-1">
                      <p className="text-[11px] font-medium tracking-wide text-[--color-muted] uppercase">
                        Symptoms
                      </p>
                      <ul className="space-y-1 text-sm text-[--color-ink]">
                        {issue.symptoms.map((symptom) => (
                          <li key={symptom} className="flex gap-2">
                            <span className="mt-1 size-1.5 shrink-0 rounded-full bg-[--color-muted]/70" />
                            <span>{symptom}</span>
                          </li>
                        ))}
                      </ul>
                    </div>
                  ) : null}

                  <div className="space-y-1">
                    <p className="text-[11px] font-medium tracking-wide text-[--color-muted] uppercase">
                      What to try
                    </p>
                    <ul className="space-y-1 text-sm text-[--color-ink]">
                      {issue.solutions.map((solution) => (
                        <li key={solution} className="flex gap-2">
                          <span className="mt-1 size-1.5 shrink-0 rounded-full bg-[--color-muted]/70" />
                          <span>{solution}</span>
                        </li>
                      ))}
                    </ul>
                  </div>

                  {issue.commands?.length ? (
                    <div className="space-y-1">
                      <p className="text-[11px] font-medium tracking-wide text-[--color-muted] uppercase">
                        Command examples
                      </p>
                      <div className="space-y-2">
                        {issue.commands.map((command) => (
                          <code
                            key={command}
                            className="block overflow-x-auto rounded-md border border-[--color-border] bg-neutral-50/80 px-3 py-2 font-mono text-[11px] text-[--color-ink] dark:bg-neutral-900/70"
                          >
                            {command}
                          </code>
                        ))}
                      </div>
                    </div>
                  ) : null}

                  {actionState?.error ? (
                    <p className="text-xs text-[--color-danger]">{actionState.error}</p>
                  ) : null}
                </article>
              );
            })}
          </DialogBody>
          <DialogFooter>
            <Button variant="outline" size="sm" onClick={() => setOpen(false)}>
              Close
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
};
