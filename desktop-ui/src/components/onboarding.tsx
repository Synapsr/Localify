import { useState } from "react";
import { Button } from "./ui/button";
import { useInstall, useStartDaemon } from "../lib/queries";
import type { BootstrapDto } from "../types/roxy";
import { cn } from "../lib/cn";

interface OnboardingProps {
  bootstrap: BootstrapDto;
}

type StepState = "idle" | "running" | "done" | "error";

export function Onboarding({ bootstrap }: OnboardingProps) {
  const install = useInstall();
  const startDaemon = useStartDaemon();
  const appName = bootstrap.environment.appName;

  const [installState, setInstallState] = useState<StepState>("idle");
  const [startState, setStartState] = useState<StepState>(
    bootstrap.daemon.running ? "done" : "idle",
  );
  const [installMessage, setInstallMessage] = useState<string | null>(null);
  const [startMessage, setStartMessage] = useState<string | null>(null);

  async function runInstall() {
    setInstallState("running");
    setInstallMessage(null);
    try {
      await install.mutateAsync();
      setInstallState("done");
    } catch (err) {
      setInstallState("error");
      setInstallMessage(err instanceof Error ? err.message : String(err));
    }
  }

  async function runStart() {
    setStartState("running");
    setStartMessage(null);
    try {
      await startDaemon.mutateAsync({ verbose: false });
      setStartState("done");
    } catch (err) {
      setStartState("error");
      setStartMessage(err instanceof Error ? err.message : String(err));
    }
  }

  const canStart = installState === "done";

  return (
    <div className="flex h-full items-center justify-center px-6 py-12">
      <div className="w-full max-w-xl space-y-10">
        <header className="space-y-3 text-center">
          <div className="inline-flex h-10 w-10 items-center justify-center rounded-xl bg-[--color-ink] text-[--color-bg] font-mono text-sm font-bold">
            {appName.charAt(0).toUpperCase()}
          </div>
          <h1 className="text-2xl font-semibold tracking-tight">
            Welcome to {appName}
          </h1>
          <p className="mx-auto max-w-md text-sm text-[--color-muted]">
            A local development proxy with custom{" "}
            <code className="rounded bg-[--color-surface] px-1.5 py-0.5 font-mono text-xs">
              {bootstrap.environment.fixedDomainSuffix}
            </code>{" "}
            domains and automatic HTTPS. Two steps to get started.
          </p>
        </header>

        <ol className="space-y-3">
          <StepCard
            index={1}
            title="Install system trust"
            description={`Generate a local certificate authority, trust it for this macOS account and its browsers, configure DNS, and install the privileged ${appName} helper. macOS asks for administrator approval once, then the desktop app reuses that helper.`}
            state={installState}
            message={installMessage}
            action={
              <Button
                variant={installState === "done" ? "secondary" : "primary"}
                onClick={runInstall}
                loading={installState === "running"}
                disabled={installState === "done"}
              >
                {installState === "done" ? "Installed" : "Install"}
              </Button>
            }
          />
          <StepCard
            index={2}
            title="Start the daemon"
            description="Launch the background proxy that routes requests to your apps."
            state={startState}
            message={startMessage}
            disabled={!canStart}
            action={
              <Button
                variant={startState === "done" ? "secondary" : "primary"}
                onClick={runStart}
                loading={startState === "running"}
                disabled={!canStart || startState === "done"}
              >
                {startState === "done" ? "Running" : "Start"}
              </Button>
            }
          />
        </ol>

        {startState === "done" && installState === "done" && (
          <div className="text-center text-sm text-[--color-muted]">
            All set. Continuing to the dashboard…
          </div>
        )}
      </div>
    </div>
  );
}

interface StepCardProps {
  index: number;
  title: string;
  description: string;
  state: StepState;
  message: string | null;
  action: React.ReactNode;
  disabled?: boolean;
}

function StepCard({
  index,
  title,
  description,
  state,
  message,
  action,
  disabled,
}: StepCardProps) {
  return (
    <li
      className={cn(
        "flex items-start gap-4 rounded-xl border border-[--color-border] bg-[--color-surface] p-5 transition-opacity",
        disabled && "opacity-50",
      )}
    >
      <div
        className={cn(
          "mt-0.5 flex h-7 w-7 shrink-0 items-center justify-center rounded-full border text-xs font-medium",
          state === "done"
            ? "border-transparent bg-[--color-accent] text-[--color-accent-fg]"
            : "border-[--color-border] text-[--color-muted]",
        )}
      >
        {state === "done" ? "✓" : index}
      </div>
      <div className="flex-1 space-y-1">
        <div className="font-medium text-[--color-ink]">{title}</div>
        <p className="text-sm text-[--color-muted]">{description}</p>
        {state === "error" && message && (
          <p className="pt-1 text-xs text-[--color-danger]">{message}</p>
        )}
      </div>
      <div className="shrink-0">{action}</div>
    </li>
  );
}
