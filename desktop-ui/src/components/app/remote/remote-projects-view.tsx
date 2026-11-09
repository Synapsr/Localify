"use client";

import { AnimatePresence, motion } from "motion/react";
import { type ReactNode, useEffect, useId, useMemo, useState } from "react";

import type { RemoteInventoryItemDto, RemoteMachineDto } from "@/types/localify";

import { ChevronDownIcon, InfoCircleIcon, SpinnerIcon } from "@/components/icons";
import {
  describeMachine,
  groupInventory,
  isFinalCode,
  remoteErrorMessage,
  type RemoteEnvironmentGroup,
  type RemoteProjectGroup,
  serviceKey,
  serviceTone,
} from "@/lib/util.remote";
import { cn } from "@/lib/utils";
import { useTerminal } from "@/providers/terminal-provider";

import { RemoteOutputPanel } from "./remote-output-panel";
import { RemoteServiceChip } from "./remote-service-chip";
import { useRemoteInventory } from "./use-remote-inventory";

interface RemoteProjectsViewProps {
  machine: RemoteMachineDto;
  query: string;
}

/**
 * Another machine's Projects screen, drawn like this Mac's and read-only
 * (docs/remote-machines.md, "A remote machine's view").
 */
export function RemoteProjectsView({ machine, query }: RemoteProjectsViewProps) {
  const state = useRemoteInventory(machine.id);
  const { closePanel, isOpen: terminalOpen, panelHeight } = useTerminal();
  const [outputKey, setOutputKey] = useState<string | null>(null);
  // The terminal takes the bottom of the window back when it opens.
  useEffect(() => {
    if (terminalOpen) setOutputKey(null);
  }, [terminalOpen]);
  const inventory = state.inventory;
  const name = inventory?.machine.name ?? machine.name;
  const groups = useMemo(() => groupInventory(inventory?.items ?? [], query), [inventory, query]);
  const outputItem = inventory?.items.find((item) => serviceKey(item) === outputKey) ?? null;

  const showOutput = (item: RemoteInventoryItemDto) => {
    // One panel at the bottom at a time, like the terminal.
    closePanel();
    setOutputKey(serviceKey(item));
  };

  return (
    <div className="flex flex-col gap-2 pb-(--bottom-padding-navigation)">
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
        <RemoteProjectCard
          key={project.id}
          project={project}
          machineName={name}
          onOutput={showOutput}
        />
      ))}

      {outputItem ? (
        <>
          {/* Keeps the last cards scrollable above the panel. */}
          <div aria-hidden style={{ height: panelHeight }} />
          <RemoteOutputPanel
            key={`${machine.id}/${outputKey}`}
            machineId={machine.id}
            machineName={name}
            item={outputItem}
            onClose={() => setOutputKey(null)}
          />
        </>
      ) : null}
    </div>
  );
}

function RemoteProjectCard({
  project,
  machineName,
  onOutput,
}: {
  project: RemoteProjectGroup;
  machineName: string;
  onOutput: (item: RemoteInventoryItemDto) => void;
}) {
  const primary = project.environments.filter((environment) => environment.primary);
  const others = project.environments.filter((environment) => !environment.primary);

  return (
    <section className="flex gap-3 rounded-xl border border-border/30 bg-neutral-50/50 px-3 py-3 transition-colors hover:border-border dark:bg-neutral-800/50">
      {/* Where this Mac's cards hold their drag handle, so both screens line up. */}
      <div className="size-9 shrink-0" />
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <h2 className="truncate text-lg font-medium">{project.name}</h2>
        {primary.length > 0 ? (
          <div className="flex flex-wrap items-center gap-1.5">
            {primary.flatMap((environment) =>
              environment.services.map((item) => (
                <RemoteServiceChip
                  key={serviceKey(item)}
                  item={item}
                  machineName={machineName}
                  onOutput={() => onOutput(item)}
                />
              )),
            )}
          </div>
        ) : null}
        {others.length > 0 ? (
          <div
            className={cn(
              "flex flex-col gap-0.5",
              primary.length > 0 && "mt-1.5 border-t border-border/60 pt-1.5",
            )}
          >
            {others.map((environment) => (
              <RemoteEnvironment
                key={environment.key}
                environment={environment}
                machineName={machineName}
                onOutput={onOutput}
              />
            ))}
          </div>
        ) : null}
      </div>
    </section>
  );
}

/** A worktree's services, folded under one line like this Mac's environments. */
function RemoteEnvironment({
  environment,
  machineName,
  onOutput,
}: {
  environment: RemoteEnvironmentGroup;
  machineName: string;
  onOutput: (item: RemoteInventoryItemDto) => void;
}) {
  const [open, setOpen] = useState(false);
  const total = environment.services.length;
  const answering = environment.services.filter((item) => serviceTone(item) === "ok").length;
  const branch =
    environment.branch && environment.branch !== environment.name ? environment.branch : null;
  const routesId = useId();

  return (
    <div>
      <button
        type="button"
        className="group/env flex w-full min-w-0 items-center gap-2 rounded-md py-1 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
        aria-expanded={open}
        aria-controls={routesId}
        onClick={() => setOpen((current) => !current)}
      >
        <ChevronDownIcon
          className={cn(
            "size-3.5 shrink-0 text-muted-foreground/60 transition-[transform,color] duration-200 ease-out group-hover/env:text-foreground",
            !open && "-rotate-90",
          )}
        />
        <span
          aria-hidden
          className={cn(
            "size-1.5 shrink-0 rounded-full",
            answering === total ? "bg-success" : answering > 0 ? "bg-amber-500" : "bg-neutral-400",
          )}
        />
        <span className="shrink-0 rounded-full border border-border/60 bg-muted/60 px-1.5 py-px text-[10px] font-medium text-muted-foreground">
          worktree
        </span>
        <span className="min-w-0 truncate text-sm font-medium">{environment.name}</span>
        {branch ? (
          <span className="hidden min-w-0 truncate text-xs text-muted-foreground sm:inline">
            {branch}
          </span>
        ) : null}
        <span className="ml-auto shrink-0 font-mono text-xs text-muted-foreground tabular-nums">
          {answering}/{total}
        </span>
      </button>
      <AnimatePresence initial={false}>
        {open ? (
          <motion.div
            key="services"
            id={routesId}
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.22, ease: [0.2, 0, 0, 1] }}
            className="overflow-hidden"
          >
            <div className="ml-[7px] flex flex-col gap-2 border-l border-border/60 py-1.5">
              {environment.services.map((item) => (
                <div key={serviceKey(item)} className="flex min-w-0 items-center gap-1.5 pl-3">
                  {total > 1 ? (
                    <span className="shrink-0 text-sm text-foreground">{item.service.name}</span>
                  ) : null}
                  <RemoteServiceChip
                    item={item}
                    machineName={machineName}
                    onOutput={() => onOutput(item)}
                  />
                </div>
              ))}
            </div>
          </motion.div>
        ) : null}
      </AnimatePresence>
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
