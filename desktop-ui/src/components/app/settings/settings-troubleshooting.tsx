"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { toast } from "sonner";

import { ChevronDownIcon, DangerTriangleIcon, InfoCircleIcon } from "@/components/icons";
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
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { bootstrapQueries, daemonMutations, systemMutations } from "@/lib/queries";
import { getInstallWarningMessage } from "@/lib/util.install-result";
import { cn } from "@/lib/utils";

import { SettingsCard } from "./settings-card";
import {
  buildTroubleshootingIssues,
  type TroubleshootingAction,
  type TroubleshootingIssue,
} from "./settings-troubleshooting.data";

async function copyTroubleshootingError(errorMessage: string) {
  await navigator.clipboard.writeText(errorMessage);
  toast.success("Error copied");
}

function showTroubleshootingFailureToast(errorMessage: string) {
  toast.error("Action failed", {
    description: errorMessage,
    action: {
      label: "Copy error",
      onClick: () => void copyTroubleshootingError(errorMessage),
    },
  });
}

export const SettingsTroubleshooting = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const install = useMutation(systemMutations.install(queryClient));
  const setupCliAccess = useMutation(systemMutations.setupCliAccess(queryClient));
  const setupNodeTrust = useMutation(systemMutations.setupNodeTrust(queryClient));
  const trustCa = useMutation(systemMutations.trustCa(queryClient));
  const trustIosSimulators = useMutation(systemMutations.trustIosSimulators());
  const startDaemon = useMutation(daemonMutations.start(queryClient));
  const [open, setOpen] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const issues = useMemo(() => (data ? buildTroubleshootingIssues(data) : []), [data]);

  if (!data) return null;

  const daemonRunning = data.daemon.running;

  async function runTroubleshootingAction(action: TroubleshootingAction) {
    setActionError(null);

    try {
      if (action === "trustCa") {
        await trustCa.mutateAsync();
        toast.success("HTTPS is trusted");
        return;
      }

      if (action === "install") {
        const result = await install.mutateAsync();
        const warning = getInstallWarningMessage(result);

        if (warning) {
          setActionError(warning);
          showTroubleshootingFailureToast(warning);
          return;
        }

        if (!daemonRunning) {
          await startDaemon.mutateAsync({ verbose: false });
        }

        toast.success("HTTPS repair completed");
        return;
      }

      if (action === "setupCliAccess") {
        await setupCliAccess.mutateAsync();
        toast.success("CLI command installed");
        return;
      }

      if (action === "setupNodeTrust") {
        await setupNodeTrust.mutateAsync();
        toast.success("Node trust repaired");
        return;
      }

      if (action === "trustIosSimulators") {
        const result = await trustIosSimulators.mutateAsync();
        if (result.status !== "success") {
          setActionError(result.message);
          showTroubleshootingFailureToast(result.message);
          return;
        }

        toast.success("iOS Simulator trust repaired", { description: result.message });
        return;
      }

      await startDaemon.mutateAsync({ verbose: false });
      toast.success("Background service started");
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      setActionError(message);
      showTroubleshootingFailureToast(message);
    }
  }

  const pendingAction =
    (install.isPending && "install") ||
    (setupCliAccess.isPending && "setupCliAccess") ||
    (setupNodeTrust.isPending && "setupNodeTrust") ||
    (trustCa.isPending && "trustCa") ||
    (trustIosSimulators.isPending && "trustIosSimulators") ||
    (startDaemon.isPending && "startDaemon") ||
    null;

  return (
    <>
      <SettingsCard
        icon={<InfoCircleIcon />}
        title="Troubleshooting"
        description="Known Localify issues, how to recognize them, and what to try next."
        action={
          <Button size="lg" variant="outline" onClick={() => setOpen(true)}>
            Open guide
          </Button>
        }
      />

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-h-[calc(100vh-2rem)] overflow-y-auto overscroll-none sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>Troubleshooting</DialogTitle>
            <DialogDescription>
              Common Localify situations, how to recognize them, and what to try next.
            </DialogDescription>
          </DialogHeader>
          <DialogBody className="pb-4">
            <ul className="divide-y divide-[--color-border]/40 overflow-hidden rounded-xl border border-[--color-border]/60">
              {issues.map((issue) => (
                <FaqRow
                  key={issue.id}
                  issue={issue}
                  open={expanded === issue.id}
                  actionError={actionError}
                  pendingAction={pendingAction}
                  onRunAction={runTroubleshootingAction}
                  onToggle={() =>
                    setExpanded((current) => (current === issue.id ? null : issue.id))
                  }
                />
              ))}
            </ul>
          </DialogBody>
          <DialogFooter showCloseButton />
        </DialogContent>
      </Dialog>
    </>
  );
};

interface FaqRowProps {
  issue: TroubleshootingIssue;
  open: boolean;
  actionError: string | null;
  pendingAction: TroubleshootingAction | null;
  onRunAction: (action: TroubleshootingAction) => void;
  onToggle: () => void;
}

const FaqRow = ({
  issue,
  open,
  actionError,
  pendingAction,
  onRunAction,
  onToggle,
}: FaqRowProps) => {
  const panelId = `troubleshoot-${issue.id}`;
  const action = issue.action;
  const agentPrompt = issue.agentPrompt;
  const actionPending = action ? pendingAction === action.id : false;
  const { copy: copyAgentPrompt, copied: agentPromptCopied } = useCopyToClipboard();

  return (
    <li className={cn(open && "bg-[--color-border]/10")}>
      <button
        type="button"
        aria-expanded={open}
        aria-controls={panelId}
        onClick={onToggle}
        className="group flex w-full items-center gap-4 px-5 py-4 text-left transition-colors hover:bg-[--color-border]/15 focus-visible:bg-[--color-border]/20 focus-visible:outline-none"
      >
        <span className="min-w-0 flex-1 text-[15px] leading-snug font-semibold text-[--color-ink]">
          {issue.title}
        </span>
        <ChevronDownIcon
          className={cn(
            "size-5 shrink-0 text-[--color-muted]/70 transition-transform duration-300 ease-out group-hover:text-[--color-ink]",
            open && "rotate-180",
          )}
        />
      </button>

      <div
        id={panelId}
        role="region"
        className={cn(
          "grid transition-[grid-template-rows] duration-300 ease-out",
          open ? "grid-rows-[1fr]" : "grid-rows-[0fr]",
        )}
      >
        <div className="overflow-hidden">
          <div className="space-y-3.5 px-5 pt-1 pb-4 text-[13.5px] leading-relaxed ">
            <p className="text-[--color-ink]/85">{issue.summary}</p>

            {issue.symptoms.length > 0 ? (
              <Section label="Symptoms">
                <ul className="space-y-1">
                  {issue.symptoms.map((item) => (
                    <li key={item} className="flex gap-2.5">
                      <span
                        className="mt-[9px] size-1 shrink-0 rounded-full bg-muted-foreground/60"
                        aria-hidden
                      />
                      <FormattedText text={item} />
                    </li>
                  ))}
                </ul>
              </Section>
            ) : null}

            {issue.solutions.length > 0 ? (
              <Section label="What to try">
                <ol className="space-y-1.5">
                  {issue.solutions.map((item, index) => (
                    <li key={item} className="flex gap-3">
                      <span
                        className="mt-px grid size-5 shrink-0 place-items-center rounded-full bg-[--color-border]/40 text-[11px] font-semibold text-[--color-ink]/70 tabular-nums"
                        aria-hidden
                      >
                        {index + 1}
                      </span>
                      <FormattedText text={item} className="pt-px" />
                    </li>
                  ))}
                </ol>
              </Section>
            ) : null}

            {issue.commands?.length ? (
              <Section label="Commands">
                <div className="space-y-1">
                  {issue.commands.map((command) => (
                    <span
                      key={command}
                      className="block overflow-x-auto rounded-md border border-[--color-border]/40 bg-[--color-border]/20 px-3 py-1.5 text-[13px] leading-relaxed text-[--color-ink]"
                    >
                      {command}
                    </span>
                  ))}
                </div>
              </Section>
            ) : null}

            {agentPrompt ? (
              <Section label="Agent prompt">
                <div className="space-y-2 rounded-md border border-[--color-border]/40 bg-[--color-border]/15 p-3">
                  <pre className="max-h-48 overflow-auto font-mono text-[12px] leading-relaxed whitespace-pre-wrap text-[--color-ink]/85">
                    {agentPrompt.body}
                  </pre>
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => void copyAgentPrompt(agentPrompt.body)}
                  >
                    {agentPromptCopied ? "Copied" : agentPrompt.label}
                  </Button>
                </div>
              </Section>
            ) : null}

            {action ? (
              <div className="flex flex-wrap items-center gap-x-3 gap-y-2 pt-0.5">
                <Button
                  size="sm"
                  variant="outline"
                  loading={actionPending}
                  onClick={() => onRunAction(action.id)}
                >
                  {action.label}
                </Button>
              </div>
            ) : null}

            {actionError && action ? (
              <div className="flex gap-2 rounded-md border border-[--color-danger]/30 bg-[--color-danger]/5 px-3 py-2 text-xs text-[--color-danger]">
                <DangerTriangleIcon className="mt-px size-4 shrink-0" />
                <p className="whitespace-pre-line">{actionError}</p>
              </div>
            ) : null}

            <p className="border-t border-[--color-border]/40 pt-2.5 text-xs text-[--color-muted]/70">
              Still stuck? Contact{" "}
              <a
                href="mailto:support@localify.dev"
                className="text-[--color-muted] underline decoration-[--color-border] underline-offset-2 transition-colors hover:text-[--color-ink] hover:decoration-[--color-muted]"
              >
                support@localify.dev
              </a>
              .
            </p>
          </div>
        </div>
      </div>
    </li>
  );
};

const Section = ({ label, children }: { label: string; children: React.ReactNode }) => (
  <div className="space-y-1.5">
    <p className="text-[12px] font-semibold text-[--color-ink]/75">{label}</p>
    {children}
  </div>
);

const FormattedText = ({ text, className }: { text: string; className?: string }) => {
  const parts = text.split(/(`[^`]+`)/g);
  return (
    <span className={className}>
      {parts.map((part, index) => {
        if (part.startsWith("`") && part.endsWith("`")) {
          return (
            <span
              key={`${part}-${index}`}
              className="rounded bg-[--color-border]/35 px-1 py-px text-[--color-ink]"
            >
              {part.slice(1, -1)}
            </span>
          );
        }
        return <span key={`${part}-${index}`}>{part}</span>;
      })}
    </span>
  );
};
