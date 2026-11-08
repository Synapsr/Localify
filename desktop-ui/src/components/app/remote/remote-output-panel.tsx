"use client";

import { useEffect, useMemo, useRef, useState } from "react";

import type { RemoteInventoryItemDto } from "@/types/localify";

import { PauseIcon, PlayIcon, SearchIcon, SpinnerIcon, XIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { formatClock, remoteErrorMessage } from "@/lib/util.remote";
import { filterOutput } from "@/lib/util.remote-output";
import { cn } from "@/lib/utils";

import { type OutputStatus, useRemoteOutput } from "./use-remote-output";

interface RemoteOutputPanelProps {
  machineId: string;
  machineName: string;
  item: RemoteInventoryItemDto;
  onClose: () => void;
}

/**
 * A service's output on another machine: read-only plain lines, selectable, with search and
 * Follow / Pause (docs/remote-machines.md, "Showing it"). Never a terminal.
 */
export function RemoteOutputPanel({
  machineId,
  machineName,
  item,
  onClose,
}: RemoteOutputPanelProps) {
  const [paused, setPaused] = useState(false);
  const [query, setQuery] = useState("");
  const runId = item.output.available ? (item.output.runId ?? null) : null;
  const { buffer, status } = useRemoteOutput(machineId, runId, paused);
  const scroller = useRef<HTMLDivElement>(null);
  const shown = useMemo(() => filterOutput(buffer.entries, query), [buffer.entries, query]);
  const partial = query ? null : buffer.partial;

  // Following keeps the newest line in view; paused, lines arrive without scrolling.
  useEffect(() => {
    if (paused || !scroller.current) return;
    scroller.current.scrollTop = scroller.current.scrollHeight;
  }, [shown, partial, paused]);

  return (
    <section
      aria-label={`Output · ${item.service.name} · ${machineName}`}
      className="flex h-80 shrink-0 flex-col overflow-hidden rounded-xl border border-border/60 bg-neutral-950 text-neutral-200 shadow-lg"
    >
      <header className="flex h-10 shrink-0 items-center gap-2 border-b border-white/10 bg-neutral-900 pr-2 pl-3">
        <h2 className="min-w-0 flex-1 truncate text-xs font-medium text-neutral-300">
          Output · {item.service.name} · {machineName}
        </h2>
        <InputGroup className="h-7 w-48 border-white/10 bg-white/5 text-neutral-200">
          <InputGroupInput
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Search output"
            aria-label="Search output"
            className="text-xs"
          />
          <InputGroupAddon>
            <SearchIcon className="text-neutral-500" />
          </InputGroupAddon>
        </InputGroup>
        {status.kind === "ended" || status.kind === "failed" ? null : (
          <Button
            type="button"
            size="xs"
            variant="ghost"
            className="text-neutral-200 hover:bg-white/10 hover:text-white"
            aria-pressed={!paused}
            onClick={() => setPaused((current) => !current)}
          >
            {paused ? <PlayIcon /> : <PauseIcon />}
            {paused ? "Follow" : "Pause"}
          </Button>
        )}
        <Button
          type="button"
          size="icon-xs"
          variant="ghost"
          className="text-neutral-300 hover:bg-white/10 hover:text-white"
          aria-label="Close output"
          onClick={onClose}
        >
          <XIcon className="size-4" />
        </Button>
      </header>
      <StatusBar status={status} item={item} machineName={machineName} />
      <div
        ref={scroller}
        className="min-h-0 flex-1 overflow-y-auto px-3 py-2 font-mono text-xs leading-5 select-text"
      >
        {shown.map((entry) =>
          entry.kind === "notice" ? (
            <div key={entry.key} className="my-1 text-[11px] text-neutral-500 italic">
              — {entry.text} —
            </div>
          ) : (
            <div key={entry.key} className="break-all whitespace-pre-wrap">
              {entry.text || " "}
            </div>
          ),
        )}
        {partial !== null ? <div className="break-all whitespace-pre-wrap">{partial}</div> : null}
        {shown.length === 0 && partial === null ? (
          <div className="text-neutral-500">
            {query ? "No line matches." : status.kind === "following" ? "No output yet." : null}
          </div>
        ) : null}
      </div>
    </section>
  );
}

function StatusBar({
  status,
  item,
  machineName,
}: {
  status: OutputStatus;
  item: RemoteInventoryItemDto;
  machineName: string;
}) {
  const exit = item.run?.exit;
  const message = (() => {
    switch (status.kind) {
      case "connecting":
        return { text: `Reading the output on ${machineName}…`, busy: true, tone: "muted" };
      case "reconnecting":
        return { text: "Reconnecting…", busy: true, tone: "muted" };
      case "idle":
        return {
          text: "Paused for a while: following again when you come back.",
          busy: false,
          tone: "muted",
        };
      case "ended":
        if (status.interrupted) {
          return {
            text: `Output was cut off when Localify stopped on ${machineName}.`,
            busy: false,
            tone: "warn",
          };
        }
        return {
          text: exit
            ? `Exited${exit.code === null ? "" : ` with code ${exit.code}`} at ${formatClock(exit.at)}`
            : "The run ended.",
          busy: false,
          tone: "muted",
        };
      case "gone":
        return { text: "The service restarted. Reading its new run…", busy: true, tone: "muted" };
      case "failed":
        return {
          text: remoteErrorMessage(status.code, machineName, status.message),
          busy: false,
          tone: "warn",
        };
      case "following":
        return null;
    }
  })();
  if (!message) return null;
  return (
    <div
      role="status"
      className={cn(
        "flex shrink-0 items-center gap-2 border-b px-3 py-1.5 text-xs",
        message.tone === "warn"
          ? "border-amber-500/20 bg-amber-500/10 text-amber-200"
          : "border-white/10 bg-white/5 text-neutral-400",
      )}
    >
      {message.busy ? <SpinnerIcon className="size-3.5 animate-spin" /> : null}
      {message.text}
    </div>
  );
}
