"use client";

import { RiCheckLine, RiFileCopyLine } from "@remixicon/react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { TerminalIcon } from "@/components/icons";
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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { bootstrapQueries, systemMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { SettingsCard } from "./settings-card";

interface CliCommand {
  command: string;
  description: string;
}

interface CliGroup {
  title: string;
  description: string;
  commands: CliCommand[];
}

const buildGroups = (cli: string): CliGroup[] => [
  {
    title: "Projects",
    description: "Bootstrap and manage local projects.",
    commands: [
      {
        command: `${cli} init-project`,
        description: "Run inside a project folder. Prompts for a domain and target.",
      },
      {
        command: `${cli} init-project my-app --domain my-app --target 3000`,
        description: "Non-interactive — pass everything for CI or scripts.",
      },
      {
        command: `${cli} list-projects`,
        description: "Show every project and its attached domains.",
      },
      {
        command: `${cli} remove-project <id> --delete-domains`,
        description: "Remove a project and unregister its domains.",
      },
    ],
  },
  {
    title: "Domains & routes",
    description: "Wire raw domains without going through a project.",
    commands: [
      {
        command: `${cli} register myapp.localify -r /=3000`,
        description: "Register a domain and forward the root path to a port.",
      },
      {
        command: `${cli} register myapp.localify --wildcard -r /=3000`,
        description: "Same plus *.myapp.localify subdomains.",
      },
      {
        command: `${cli} list`,
        description: "List every registered domain.",
      },
      {
        command: `${cli} unregister myapp.localify`,
        description: "Remove a domain.",
      },
    ],
  },
  {
    title: "Daemon",
    description: "Control the background proxy.",
    commands: [
      {
        command: `${cli} status`,
        description: "Daemon health, ports, and registered domains.",
      },
      {
        command: `${cli} restart`,
        description: "Restart after editing the config by hand.",
      },
      {
        command: `${cli} reload`,
        description: "Hot-reload the config without dropping connections.",
      },
    ],
  },
  {
    title: "Logs",
    description: "Inspect what the proxy is doing.",
    commands: [
      {
        command: `${cli} logs -f`,
        description: "Follow the live log stream (Ctrl-C to stop).",
      },
      {
        command: `${cli} logs -n 200`,
        description: "Print the last 200 lines.",
      },
      {
        command: `${cli} logs --clear`,
        description: "Wipe stored logs.",
      },
    ],
  },
];

export const SettingsCli = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const setupCliAccess = useMutation(systemMutations.setupCliAccess(queryClient));
  const [cheatsheetOpen, setCheatsheetOpen] = useState(false);

  if (!data) return null;

  const appName = data.environment.appName;
  const cliCommand = appName.toLowerCase();
  const cliReady = data.environment.cliAccessReady;
  const cliError = setupCliAccess.error instanceof Error ? setupCliAccess.error.message : null;
  const groups = buildGroups(cliCommand);

  return (
    <>
      <SettingsCard
        icon={<TerminalIcon />}
        title={cliReady ? `${cliCommand} is in your shell` : `Install ${cliCommand} in your shell`}
        description={
          cliReady
            ? `Run ${cliCommand} from any terminal, VS Code task, or CI script.`
            : `Adds a small symlink so ${cliCommand} works from any shell.`
        }
        action={
          cliReady ? (
            <Button size="lg" variant="outline" onClick={() => setCheatsheetOpen(true)}>
              Cheatsheet
            </Button>
          ) : (
            <Button
              size="lg"
              variant="primary"
              loading={setupCliAccess.isPending}
              onClick={() => setupCliAccess.mutate()}
            >
              Install
            </Button>
          )
        }
      />
      {cliError ? <p className="px-1 text-xs text-[--color-danger]">{cliError}</p> : null}

      <Dialog open={cheatsheetOpen} onOpenChange={setCheatsheetOpen}>
        <DialogContent className="max-h-[80vh] overflow-y-auto sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>{cliCommand} cheatsheet</DialogTitle>
            <DialogDescription>
              Run{" "}
              <code className="rounded bg-[--color-border]/40 px-1 py-0.5 font-mono text-[11px]">
                {cliCommand} --help
              </code>{" "}
              in any shell to see every flag for every command.
            </DialogDescription>
          </DialogHeader>
          <DialogBody className="space-y-6 pb-6">
            <section className="space-y-2">
              <p className="text-[10px] font-medium tracking-wider text-[--color-muted] uppercase">
                Installed at
              </p>
              <CliCommandLine command={data.environment.cliCommandPath} />
            </section>
            {groups.map((group) => (
              <section key={group.title} className="space-y-3">
                <header className="space-y-0.5">
                  <h4 className="text-[13px] font-semibold text-[--color-ink]">{group.title}</h4>
                  <p className="text-[11px] text-[--color-muted]">{group.description}</p>
                </header>
                <ul className="space-y-2.5">
                  {group.commands.map((entry) => (
                    <li key={entry.command} className="space-y-1">
                      <CliCommandLine command={entry.command} />
                      <p className="pl-1 text-[11px] text-[--color-muted]">{entry.description}</p>
                    </li>
                  ))}
                </ul>
              </section>
            ))}
          </DialogBody>
          <DialogFooter showCloseButton />
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
