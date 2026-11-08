"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { toast } from "sonner";

import type { RemoteMachineDto } from "@/types/localify";

import { CheckIcon, ChevronDownIcon, RefreshIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cloudMutations, remoteMutations, remoteQueries } from "@/lib/queries";
import {
  describeMachine,
  describeRelay,
  type RemoteTone,
  SETTINGS_REMOTE_ANCHOR,
} from "@/lib/util.remote";
import { cn } from "@/lib/utils";
import { useRemote } from "@/providers/remote-provider";

export const remoteToneDot: Record<RemoteTone, string> = {
  ok: "bg-success",
  pending: "bg-amber-500",
  warn: "bg-amber-500",
  error: "bg-destructive",
  off: "bg-neutral-400",
};

/** The other machines of the account, for those who have any. */
export function useOtherMachines(): RemoteMachineDto[] {
  const machines = useQuery(remoteQueries.machines());
  return machines.data?.filter((machine) => !machine.self) ?? [];
}

/**
 * Picks the machine the Projects screen shows: this Mac, or a paired one
 * (docs/remote-machines.md, "Machine switcher"). Hidden while the account has no other machine,
 * so the app is unchanged for those without remote access.
 */
export function MachineSwitcher() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const status = useQuery(remoteQueries.status());
  const others = useOtherMachines();
  const { selectedMachineId, selectMachine, startPairing } = useRemote();
  const retry = useMutation(cloudMutations.refreshTunnel(queryClient));
  const unpair = useMutation(remoteMutations.unpair(queryClient));
  if (others.length === 0) return null;

  const selected = others.find((machine) => machine.id === selectedMachineId) ?? null;
  const relay = status.data ? describeRelay(status.data) : null;
  const selectedTone = selected ? describeMachine(selected).tone : (relay?.tone ?? "off");

  const act = (machine: RemoteMachineDto) => {
    const view = describeMachine(machine);
    if (view.viewable) return selectMachine(machine.id);
    switch (view.action) {
      case "pair":
        return startPairing(machine);
      case "unpair":
        return unpair.mutate(
          { machineId: machine.id },
          {
            onError: (error) =>
              toast.error(`Could not unpair ${machine.name}`, { description: error.message }),
          },
        );
      case "upgrade":
        return void navigate({ to: "/settings", hash: SETTINGS_REMOTE_ANCHOR });
      default:
        return undefined;
    }
  };

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button variant="outline" size="sm" className="gap-2 border-dashed" aria-label="Machine">
            <span
              aria-hidden
              className={cn("size-2 shrink-0 rounded-full", remoteToneDot[selectedTone])}
            />
            <span className="max-w-48 truncate">{selected?.name ?? "This Mac"}</span>
            <ChevronDownIcon className="size-3.5 text-muted-foreground" />
          </Button>
        }
      />
      <DropdownMenuContent align="start" className="w-80">
        {relay ? (
          <div className="flex items-start gap-2 px-2 py-1.5">
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-1.5 text-xs font-medium text-foreground">
                <span
                  aria-hidden
                  className={cn("size-1.5 rounded-full", remoteToneDot[relay.tone])}
                />
                {relay.label}
              </div>
              {relay.detail ? (
                <div className="mt-0.5 text-xs leading-normal text-muted-foreground">
                  {relay.detail}
                </div>
              ) : null}
            </div>
            {relay.retry ? (
              <Button
                size="xs"
                variant="ghost"
                onClick={() => retry.mutate()}
                disabled={retry.isPending}
              >
                <RefreshIcon className={retry.isPending ? "animate-spin" : undefined} />
                Retry now
              </Button>
            ) : null}
          </div>
        ) : null}
        <DropdownMenuSeparator />
        <DropdownMenuItem onClick={() => selectMachine(null)}>
          <span className="min-w-0 flex-1 truncate">This Mac</span>
          {selected ? null : <CheckIcon className="text-muted-foreground" />}
        </DropdownMenuItem>
        {others.map((machine) => (
          <MachineItem
            key={machine.id}
            machine={machine}
            selected={machine.id === selected?.id}
            onSelect={() => act(machine)}
          />
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function MachineItem({
  machine,
  selected,
  onSelect,
}: {
  machine: RemoteMachineDto;
  selected: boolean;
  onSelect: () => void;
}) {
  const view = describeMachine(machine);
  const action = { pair: "Pair", unpair: "Unpair", upgrade: "Upgrade" } as const;
  return (
    <DropdownMenuItem
      onClick={onSelect}
      disabled={!view.viewable && !view.action}
      className="items-start"
    >
      <span
        aria-hidden
        className={cn("mt-1.5 size-2 shrink-0 rounded-full", remoteToneDot[view.tone])}
      />
      <span className="min-w-0 flex-1">
        <span className="block truncate">{machine.name}</span>
        {view.label ? (
          <span className="block text-xs leading-normal text-muted-foreground">
            {view.label}
            {view.detail ? ` · ${view.detail}` : ""}
          </span>
        ) : null}
      </span>
      {view.action && !view.viewable ? (
        <span className="mt-0.5 shrink-0 rounded-sm bg-foreground/10 px-1.5 py-0.5 text-[11px] font-medium">
          {action[view.action]}
        </span>
      ) : null}
      {selected ? <CheckIcon className="mt-0.5 text-muted-foreground" /> : null}
    </DropdownMenuItem>
  );
}
