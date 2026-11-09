"use client";

import { useQuery } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { toast } from "sonner";

import type { RemoteControlAction, RemoteInventoryItemDto } from "@/types/localify";

import { PublicUrlBadge } from "@/components/app/projects/public-url/public-url-badge";
import {
  CheckIcon,
  CopyIcon,
  DangerTriangleIcon,
  DotsIcon,
  ExternalLinkIcon,
  PlayIcon,
  RefreshIcon,
  SpinnerIcon,
  TerminalIcon,
  XIcon,
} from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Popover, PopoverContent } from "@/components/ui/popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { bootstrapQueries } from "@/lib/queries";
import { tauriClient } from "@/lib/tauri-client";
import { ownerLabel, remoteHost, serviceChips, serviceTone } from "@/lib/util.remote";
import { type QuickAction, quickAction } from "@/lib/util.remote-controls";
import { cn } from "@/lib/utils";

import { remoteToneDot } from "./machine-switcher";
import { useRemoteControls } from "./remote-controls";
import { RemoteQuickActionCard } from "./remote-quick-action-card";

interface RemoteServiceChipProps {
  item: RemoteInventoryItemDto;
  machineName: string;
  onOutput: () => void;
}

/**
 * A service on another machine, drawn like a domain chip of this Mac's Projects screen. Its
 * `.localify` URL resolves on that machine only, so the name opens the output instead.
 */
export function RemoteServiceChip({ item, machineName, onOutput }: RemoteServiceChipProps) {
  const { data: bootstrap } = useQuery(bootstrapQueries.read());
  const { copy, copied } = useCopyToClipboard();
  const [menuOpen, setMenuOpen] = useState(false);
  const controls = useRemoteControls();
  const chip = useRef<HTMLSpanElement>(null);
  // The card of a control that is not allowed, under the chip.
  const [card, setCard] = useState<RemoteControlAction | null>(null);
  const toggle: RemoteControlAction = item.process === "running" ? "stop" : "start";
  const controlOff = controls.down ?? controls.inFlight(item)?.text ?? null;
  const quickFor = (action: RemoteControlAction) =>
    quickAction(item, action, controls.proto, machineName, controls.appRunning);
  const restart = quickFor("restart");
  const cardQuick = card ? quickFor(card) : null;
  const press = (action: RemoteControlAction) => {
    const quick = quickFor(action);
    if (quick.kind === "run") controls.run(item, action);
    // Restart of a legacy run goes straight to its confirmation.
    else if (quick.kind === "relaunch" && action === "restart") controls.relaunch(item);
    else setCard((current) => (current === action ? null : action));
  };
  const tone = serviceTone(item);
  const publicUrl = item.publicUrl?.serving ? item.publicUrl.url : null;
  const host = remoteHost(item.localUrl);
  const outputReason = outputUnavailableMessage(item, machineName);

  const openPublicUrl = async (url: string) => {
    try {
      await tauriClient.openProjectUrl({
        url,
        browser: bootstrap?.launcherPreferences.browser ?? "default",
      });
    } catch (error) {
      toast.error("Open in browser failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  return (
    <span
      ref={chip}
      className={cn(
        "group/chip inline-flex h-8 items-center gap-1 rounded-full border border-border pr-1 pl-2 text-sm text-foreground transition-colors",
        "hover:border-foreground/20 hover:bg-muted/50",
        tone === "ok" ? "opacity-80" : "opacity-60",
      )}
    >
      <Tooltip>
        <TooltipTrigger
          render={
            <span
              tabIndex={0}
              role="img"
              aria-label={`${item.service.name}: ${statusSummary(item)}`}
              className={cn("size-4 shrink-0 rounded-full", remoteToneDot[tone])}
            />
          }
        />
        <TooltipContent>
          <div className="flex flex-col gap-0.5">
            {ownerLabel(item.owner) ? (
              <span className="font-medium">{ownerLabel(item.owner)}</span>
            ) : null}
            {serviceChips(item).map((chip) => (
              <span key={chip.name}>
                {chip.name}: {chip.value}
              </span>
            ))}
          </div>
        </TooltipContent>
      </Tooltip>

      <Tooltip>
        <TooltipTrigger
          render={
            <button
              type="button"
              onClick={() => (item.output.available ? onOutput() : void copy(item.localUrl))}
              className="inline-flex h-full items-center rounded-full px-1 hover:text-foreground"
            />
          }
        >
          <span className="truncate">{host}</span>
        </TooltipTrigger>
        <TooltipContent>
          {item.output.available
            ? `Show the output of ${item.service.name}`
            : `Copy ${host}. It opens only on ${machineName}.`}
        </TooltipContent>
      </Tooltip>

      <span className="inline-flex h-5 shrink-0 items-center rounded bg-muted px-1 text-xs text-muted-foreground">
        :{item.port.number}
      </span>

      {publicUrl ? (
        <PublicUrlBadge url={publicUrl} onOpen={() => void openPublicUrl(publicUrl)} />
      ) : null}

      {item.run?.profile ? (
        <span
          className={cn(
            "inline-flex h-5 max-w-28 shrink-0 items-center gap-1 rounded-full border px-1.5 text-[11px] font-medium",
            item.run.profileDanger
              ? "border-destructive-foreground/30 bg-destructive-foreground/10 text-destructive-foreground"
              : "border-border bg-muted text-muted-foreground",
          )}
        >
          {item.run.profileDanger ? (
            <DangerTriangleIcon aria-hidden className="size-3 shrink-0" />
          ) : null}
          <span className="truncate">{item.run.profile}</span>
        </span>
      ) : null}

      <ControlButton
        item={item}
        action={toggle}
        machineName={machineName}
        off={controlOff}
        pending={controls.inFlight(item) !== null}
        onPress={() => press(toggle)}
      />

      <Popover
        open={cardQuick !== null && cardQuick.kind !== "run"}
        onOpenChange={(open, details) => {
          if (open) return;
          // The chip's own buttons open and toggle the card, and its menu hands focus back
          // to it: neither closes the card here.
          const event = details.event;
          const target =
            details.reason === "focus-out" && event instanceof FocusEvent
              ? event.relatedTarget
              : event?.target;
          const inChip = target instanceof Node && chip.current?.contains(target);
          if (inChip && (details.reason === "outside-press" || details.reason === "focus-out")) {
            return;
          }
          setCard(null);
        }}
      >
        <PopoverContent
          anchor={chip}
          align="start"
          finalFocus={false}
          className="w-80 bg-popover p-3 text-sm text-popover-foreground ring-1 ring-foreground/10"
        >
          {card && cardQuick && cardQuick.kind !== "run" ? (
            <RemoteQuickActionCard
              item={item}
              action={card}
              quick={cardQuick}
              onDone={() => setCard(null)}
            />
          ) : null}
        </PopoverContent>
      </Popover>

      <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
        <DropdownMenuTrigger
          render={
            <Button
              type="button"
              size="icon-xs"
              variant="ghost"
              aria-label={`Actions for ${item.service.name} on ${machineName}`}
              className="ml-0.5 size-6 rounded-full text-muted-foreground hover:bg-foreground/10 hover:text-foreground aria-expanded:bg-foreground/10 aria-expanded:text-foreground"
            >
              <DotsIcon className="size-5 stroke-2" />
            </Button>
          }
        />
        <DropdownMenuContent align="end" sideOffset={6} className="w-64">
          {outputRelaunch(item, restart) ? (
            <DropdownMenuItem disabled={controlOff !== null} onClick={() => press("restart")}>
              <RefreshIcon />
              <span className="flex min-w-0 flex-col">
                Relaunch to share
                <span className="text-xs text-muted-foreground">{controlOff ?? outputReason}</span>
              </span>
            </DropdownMenuItem>
          ) : (
            <DropdownMenuItem onClick={onOutput} disabled={!item.output.available}>
              <TerminalIcon />
              <span className="flex min-w-0 flex-col">
                Show output
                {outputReason ? (
                  <span className="text-xs text-muted-foreground">{outputReason}</span>
                ) : null}
              </span>
            </DropdownMenuItem>
          )}
          {publicUrl ? (
            <DropdownMenuItem onClick={() => void openPublicUrl(publicUrl)}>
              <ExternalLinkIcon />
              Open public URL
            </DropdownMenuItem>
          ) : null}
          <DropdownMenuItem onClick={() => void copy(item.localUrl)}>
            {copied ? <CheckIcon className="text-emerald-500" /> : <CopyIcon />}
            {copied ? "URL copied" : "Copy URL"}
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem disabled={controlOff !== null} onClick={() => press("restart")}>
            <RefreshIcon />
            <span className="flex min-w-0 flex-col">
              {restart.kind === "relaunch" ? `Relaunch on ${machineName}` : "Restart"}
              {controlOff ? (
                <span className="text-xs text-muted-foreground">{controlOff}</span>
              ) : null}
            </span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </span>
  );
}

const CONTROL_LABEL: Record<RemoteControlAction, string> = {
  start: "Start",
  stop: "Stop",
  restart: "Restart",
};

/**
 * Start or Stop, where this Mac's chips put Launch or Stop. Disabled only while the session
 * is down or a control of this service is on its way; a control that is not allowed opens a
 * card that says why.
 */
function ControlButton({
  item,
  action,
  machineName,
  off,
  pending,
  onPress,
}: {
  item: RemoteInventoryItemDto;
  action: RemoteControlAction;
  machineName: string;
  /** Why it can't be pressed now. */
  off: string | null;
  pending: boolean;
  onPress: () => void;
}) {
  const label = `${CONTROL_LABEL[action]} ${item.service.name} on ${machineName}`;
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <span tabIndex={off ? 0 : -1} className="inline-flex">
            <Button
              type="button"
              size="icon-xs"
              variant="ghost"
              disabled={off !== null}
              aria-label={off ?? label}
              onClick={onPress}
              className="ml-0.5 size-6 rounded-full text-foreground hover:bg-foreground/10 hover:text-foreground"
            >
              {pending ? (
                <SpinnerIcon className="animate-spin" />
              ) : action === "stop" ? (
                <XIcon />
              ) : (
                <PlayIcon className="translate-x-px" />
              )}
            </Button>
          </span>
        }
      />
      <TooltipContent>{off ?? label}</TooltipContent>
    </Tooltip>
  );
}

/** Output's "Relaunch to share": a legacy run this viewer can relaunch from here. */
function outputRelaunch(item: RemoteInventoryItemDto, restart: QuickAction): boolean {
  return (
    !item.output.available && item.output.reason === "legacy_run" && restart.kind === "relaunch"
  );
}

/** The dot's tooltip in words, for the keyboard and screen readers. */
function statusSummary(item: RemoteInventoryItemDto): string {
  const owner = ownerLabel(item.owner);
  const states = serviceChips(item).map((chip) => `${chip.name} ${chip.value}`);
  return (owner ? [owner, ...states] : states).join(", ");
}

function outputUnavailableMessage(
  item: RemoteInventoryItemDto,
  machineName: string,
): string | null {
  if (item.output.available) return null;
  switch (item.output.reason) {
    case "terminal":
      return `It runs in a terminal on ${machineName}.`;
    case "unknown_owner":
      return `Localify didn't start it on ${machineName}.`;
    case "legacy_run":
      return `Relaunch it on ${machineName} to share its output.`;
    default:
      return "It hasn't run yet.";
  }
}
