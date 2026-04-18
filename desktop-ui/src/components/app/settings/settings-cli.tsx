"use client";

import { RiCheckLine, RiFileCopyLine } from "@remixicon/react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { CodeIcon, TerminalIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { bootstrapQueries, systemMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { SettingsGroup, SettingsRow, SettingsSection } from "./settings-list";

interface CliExample {
  label: string;
  command: string;
  hint: string;
}

export const SettingsCli = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const setupCliAccess = useMutation(systemMutations.setupCliAccess(queryClient));
  const [examplesOpen, setExamplesOpen] = useState(false);

  if (!data) return null;

  const appName = data.environment.appName;
  const cliCommand = appName.toLowerCase();
  const cliError = setupCliAccess.error instanceof Error ? setupCliAccess.error.message : null;

  const examples: CliExample[] = [
    {
      label: "Bootstrap the current project",
      command: `${cliCommand} init-project`,
      hint: "Run from a project folder to create a domain and wire up the dev script.",
    },
    {
      label: "With a frontend and an API domain",
      command: `${cliCommand} init-project --with-api`,
      hint: "Adds an extra api-<name> domain alongside the primary one.",
    },
    {
      label: "Fully scripted, no prompts",
      command: `${cliCommand} init-project my-app --domain my-app --target 3000`,
      hint: "Pass name, domain, and target for CI or non-interactive shells.",
    },
  ];

  return (
    <>
      <SettingsSection label="Command line" icon={<TerminalIcon />}>
        <SettingsGroup>
          <SettingsRow
            icon={<TerminalIcon />}
            title={cliCommand}
            description={
              data.environment.cliAccessReady
                ? "Installed and ready in your terminal."
                : "Not installed in your shell PATH yet."
            }
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
            icon={<CodeIcon />}
            title="Usage examples"
            description={`Common ${cliCommand} commands to get started.`}
            onActivate={() => setExamplesOpen(true)}
          />
        </SettingsGroup>
        {cliError ? <p className="px-1 text-xs text-[--color-danger]">{cliError}</p> : null}
      </SettingsSection>

      <Dialog open={examplesOpen} onOpenChange={setExamplesOpen}>
        <DialogContent className="sm:max-w-xl">
          <DialogHeader>
            <DialogTitle>Command line examples</DialogTitle>
            <DialogDescription>
              Run{" "}
              <code className="rounded bg-[--color-border]/40 px-1 py-0.5 font-mono text-[11px]">
                {cliCommand} --help
              </code>{" "}
              to see every command.
            </DialogDescription>
          </DialogHeader>
          <DialogBody className="space-y-4 pb-6">
            <CliCommandLine command={data.environment.cliCommandPath} />
            <ul className="space-y-3">
              {examples.map((example) => (
                <li key={example.command} className="space-y-1.5">
                  <p className="text-xs font-medium text-[--color-ink]">{example.label}</p>
                  <CliCommandLine command={example.command} />
                  <p className="text-[11px] text-[--color-muted]">{example.hint}</p>
                </li>
              ))}
            </ul>
          </DialogBody>
        </DialogContent>
      </Dialog>
    </>
  );
};

const CliCommandLine = ({ command }: { command: string }) => {
  const { copy, copied } = useCopyToClipboard();

  return (
    <div
      className={cn(
        "group flex items-center gap-2 rounded-md border border-[--color-border] bg-[--color-surface] px-2.5 py-1.5",
        "font-mono text-xs text-[--color-ink]",
      )}
    >
      <span aria-hidden className="text-[--color-muted] select-none">
        $
      </span>
      <span className="min-w-0 flex-1 truncate">{command}</span>
      <Tooltip>
        <TooltipTrigger
          render={
            <button
              type="button"
              onClick={() => copy(command)}
              aria-label={copied ? "Copied" : `Copy ${command}`}
              className="inline-flex size-6 shrink-0 items-center justify-center rounded-md text-[--color-muted] transition-colors hover:bg-[--color-border]/40 hover:text-[--color-ink]"
            />
          }
        >
          {copied ? (
            <RiCheckLine className="size-3.5 text-emerald-500" />
          ) : (
            <RiFileCopyLine className="size-3.5" />
          )}
        </TooltipTrigger>
        <TooltipContent>{copied ? "Copied" : "Copy"}</TooltipContent>
      </Tooltip>
    </div>
  );
};
