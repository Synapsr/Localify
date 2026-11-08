"use client";

import { useQuery } from "@tanstack/react-query";
import { type ReactNode, useMemo, useState } from "react";
import { toast } from "sonner";

import type {
  RemoteControlAction,
  RemoteInventoryItemDto,
  RemoteMachineDto,
} from "@/types/localify";

import {
  CheckIcon,
  CopyIcon,
  ExternalLinkIcon,
  FileTextIcon,
  InfoCircleIcon,
  PlayIcon,
  PowerIcon,
  RefreshIcon,
  SpinnerIcon,
} from "@/components/icons";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { bootstrapQueries } from "@/lib/queries";
import { tauriClient } from "@/lib/tauri-client";
import {
  describeMachine,
  groupInventory,
  isFinalCode,
  ownerLabel,
  remoteErrorMessage,
  remoteUrlLabel,
  serviceChips,
  serviceKey,
  type ServiceChip,
} from "@/lib/util.remote";
import { cn } from "@/lib/utils";

import { remoteToneDot } from "./machine-switcher";
import { RemoteOutputPanel } from "./remote-output-panel";
import { useRemoteInventory } from "./use-remote-inventory";

interface RemoteProjectsViewProps {
  machine: RemoteMachineDto;
  query: string;
}

/**
 * Another machine's Projects screen, read-only (docs/remote-machines.md, "A remote machine's
 * view"): projects, environments and services with their four states, owners and public URLs.
 */
export function RemoteProjectsView({ machine, query }: RemoteProjectsViewProps) {
  const state = useRemoteInventory(machine.id);
  const [outputKey, setOutputKey] = useState<string | null>(null);
  const inventory = state.inventory;
  const name = inventory?.machine.name ?? machine.name;
  const groups = useMemo(() => groupInventory(inventory?.items ?? [], query), [inventory, query]);
  const outputItem = inventory?.items.find((item) => serviceKey(item) === outputKey) ?? null;

  return (
    <div className="flex flex-col gap-3 pb-(--bottom-padding-navigation)">
      <Banner>
        You're viewing {name}. Its .localify URLs open only on {name}.
      </Banner>
      <MachineNotice machine={machine} />
      {state.phase === "lost" ? (
        <Banner tone="warn">
          {remoteErrorMessage(state.code, name, state.message)}
          {isFinalCode(state.code) ? null : ` Trying again in ${state.retryIn} s.`}
        </Banner>
      ) : null}
      {inventory?.truncated ? (
        <Banner>{name} has more services than fit here; some are left out.</Banner>
      ) : null}

      {!inventory && state.phase !== "lost" ? (
        <div className="flex items-center gap-2 px-1 py-6 text-sm text-muted-foreground">
          <SpinnerIcon className="size-4 animate-spin" />
          Connecting to {name}…
        </div>
      ) : null}

      {inventory && groups.length === 0 ? (
        <p className="px-1 py-6 text-sm text-muted-foreground">
          {query ? "No service matches." : `${name} has no projects yet.`}
        </p>
      ) : null}

      {groups.map((project) => (
        <section
          key={project.id}
          className="rounded-xl border border-border/30 bg-neutral-50/50 px-4 py-3 dark:bg-neutral-800/50"
        >
          <h2 className="truncate text-lg font-medium">{project.name}</h2>
          {project.environments.map((environment) => (
            <div key={environment.key} className="mt-3">
              <div className="mb-1.5 flex items-center gap-2 text-xs text-muted-foreground">
                <span className="font-medium text-foreground/80">{environment.name}</span>
                {environment.branch && environment.branch !== environment.name ? (
                  <span className="font-mono">{environment.branch}</span>
                ) : null}
                {environment.primary ? <span>primary</span> : null}
              </div>
              <ul className="divide-y divide-border/40 rounded-lg border border-border/40 bg-background">
                {environment.services.map((item) => (
                  <ServiceRow
                    key={serviceKey(item)}
                    item={item}
                    machineName={name}
                    onOutput={() => setOutputKey(serviceKey(item))}
                  />
                ))}
              </ul>
            </div>
          ))}
        </section>
      ))}

      {outputItem ? (
        <div className="sticky bottom-(--bottom-padding-navigation) z-20">
          <RemoteOutputPanel
            key={`${machine.id}/${outputKey}`}
            machineId={machine.id}
            machineName={name}
            item={outputItem}
            onClose={() => setOutputKey(null)}
          />
        </div>
      ) : null}
    </div>
  );
}

/** What the switcher says about the machine, when it is anything but connected. */
function MachineNotice({ machine }: { machine: RemoteMachineDto }) {
  const view = describeMachine(machine);
  if (!view.label || machine.state === "not_sharing") return null;
  return (
    <Banner tone="warn">
      {machine.name}: {view.label}
      {view.detail ? ` ${view.detail}` : ""}
    </Banner>
  );
}

function Banner({ children, tone = "info" }: { children: ReactNode; tone?: "info" | "warn" }) {
  return (
    <div
      role={tone === "warn" ? "status" : undefined}
      className={cn(
        "flex items-start gap-2 rounded-lg border px-3 py-2 text-xs leading-normal",
        tone === "info"
          ? "border-border/50 bg-muted/40 text-muted-foreground"
          : "border-amber-500/20 bg-amber-500/10 text-amber-800 dark:text-amber-200",
      )}
    >
      <InfoCircleIcon className="mt-0.5 size-3.5 shrink-0" />
      <span className="min-w-0 flex-1">{children}</span>
    </div>
  );
}

function ServiceRow({
  item,
  machineName,
  onOutput,
}: {
  item: RemoteInventoryItemDto;
  machineName: string;
  onOutput: () => void;
}) {
  const { data: bootstrap } = useQuery(bootstrapQueries.read());
  const browser = bootstrap?.launcherPreferences.browser ?? "default";
  const owner = ownerLabel(item.owner);
  const publicUrl = item.publicUrl?.serving ? item.publicUrl.url : null;

  const openPublicUrl = async (url: string) => {
    try {
      await tauriClient.openProjectUrl({ url, browser });
    } catch (error) {
      toast.error("Open in browser failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  return (
    <li className="flex flex-col gap-2 px-3 py-2.5">
      <div className="flex flex-wrap items-center gap-2">
        <span className="truncate text-sm font-medium">{item.service.name}</span>
        {owner ? <span className="text-xs text-muted-foreground">{owner}</span> : null}
        {item.run?.profile ? (
          <span className="rounded-sm bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground">
            {item.run.profile}
          </span>
        ) : null}
        <div className="ml-auto flex flex-wrap items-center gap-1">
          {item.output.available ? (
            <Button size="xs" variant="outline" onClick={onOutput}>
              <FileTextIcon />
              Output
            </Button>
          ) : null}
          {publicUrl ? (
            <Button size="xs" variant="outline" onClick={() => void openPublicUrl(publicUrl)}>
              <ExternalLinkIcon />
              Open public URL
            </Button>
          ) : null}
          <ControlButton item={item} action="start" machineName={machineName} />
          <ControlButton item={item} action="stop" machineName={machineName} />
          <ControlButton item={item} action="restart" machineName={machineName} />
        </div>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {serviceChips(item).map((chip) => (
          <Chip key={chip.name} chip={chip} />
        ))}
      </div>
      <LocalUrl url={item.localUrl} machineName={machineName} />
    </li>
  );
}

function Chip({ chip }: { chip: ServiceChip }) {
  return (
    <span className="inline-flex items-center gap-1.5 rounded-full border border-border/50 px-2 py-0.5 text-[11px] text-muted-foreground">
      <span aria-hidden className={cn("size-1.5 rounded-full", remoteToneDot[chip.tone])} />
      <span>{chip.name}:</span>
      <span className="text-foreground/80">{chip.value}</span>
    </span>
  );
}

/** A target's `.localify` URL: copied, never opened, since it resolves on that machine only. */
function LocalUrl({ url, machineName }: { url: string; machineName: string }) {
  const { copy, copied } = useCopyToClipboard();
  return (
    <div className="flex items-center gap-1 text-xs text-muted-foreground">
      <span className="truncate font-mono">{remoteUrlLabel(url, machineName)}</span>
      <Button
        size="icon-xs"
        variant="ghost"
        aria-label={copied ? "URL copied" : "Copy URL"}
        onClick={() => void copy(url)}
      >
        {copied ? <CheckIcon className="text-emerald-500" /> : <CopyIcon />}
      </Button>
    </div>
  );
}

const CONTROL_LABEL: Record<RemoteControlAction, string> = {
  start: "Start",
  stop: "Stop",
  restart: "Restart",
};

const CONTROL_ICON = {
  start: PlayIcon,
  stop: PowerIcon,
  restart: RefreshIcon,
} as const;

/**
 * Start, Stop and Restart follow the target's `controls`. Remote controls come in a later
 * version, so each one is disabled, and says why in its tooltip.
 */
function ControlButton({
  item,
  action,
  machineName,
}: {
  item: RemoteInventoryItemDto;
  action: RemoteControlAction;
  machineName: string;
}) {
  const control = item.controls[action];
  const Icon = CONTROL_ICON[action];
  const reason = control.code
    ? remoteErrorMessage(control.code, machineName, "Not available.")
    : `${CONTROL_LABEL[action]} from another machine comes in a later version.`;
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <span tabIndex={0} className="inline-flex">
            <Button size="icon-xs" variant="ghost" disabled aria-label={CONTROL_LABEL[action]}>
              <Icon />
            </Button>
          </span>
        }
      />
      <TooltipContent>
        {CONTROL_LABEL[action]}: {reason}
      </TooltipContent>
    </Tooltip>
  );
}
