"use client";

import { useState } from "react";
import { toast } from "sonner";

import type { TerminalSessionDto } from "@/types/localify";

import { DangerTriangleIcon, DotsIcon, RefreshIcon, TerminalIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { toTerminalTargetRequest, useTerminal } from "@/providers/terminal-provider";

/** Shown under a service tab once its run ended and its output was finalized. */
export function TerminalRunEndBar({ session }: { session: TerminalSessionDto }) {
  const { openTerminal, restartSession } = useTerminal();
  const [restarting, setRestarting] = useState(false);
  const ended = session.run?.ended;
  if (!ended) return null;

  const time = new Date(ended.at * 1000).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
  const label = ended.stopped
    ? `Stopped at ${time}`
    : `Exited with code ${ended.code ?? "unknown"} at ${time}`;

  const restart = async () => {
    setRestarting(true);
    try {
      await restartSession(session.id);
    } catch (error) {
      showError("Restart failed", error);
    } finally {
      setRestarting(false);
    }
  };

  const openShell = async () => {
    try {
      await openTerminal(toTerminalTargetRequest(session.target));
    } catch (error) {
      showError("Open terminal failed", error);
    }
  };

  return (
    <div className="absolute inset-x-0 bottom-0 z-10 flex flex-wrap items-center gap-2 border-t border-white/10 bg-[#171717] px-3 py-2 text-xs text-neutral-300">
      <span className="min-w-0 flex-1 truncate">
        {label}
        {ended.error ? <span className="ml-2 text-red-400">{ended.error}</span> : null}
      </span>
      <Button
        type="button"
        size="xs"
        variant="ghost"
        className="text-neutral-200 hover:bg-white/10 hover:text-white"
        disabled={restarting}
        onClick={() => void restart()}
      >
        <RefreshIcon className={restarting ? "animate-spin" : undefined} />
        Restart
      </Button>
      <Button
        type="button"
        size="xs"
        variant="ghost"
        className="text-neutral-200 hover:bg-white/10 hover:text-white"
        onClick={() => void openShell()}
      >
        <TerminalIcon />
        Open a terminal here
      </Button>
    </div>
  );
}

/** Said in the panel when services start without what the user's shell config sets up. */
export function ShellEnvironmentWarning() {
  const { reloadShellEnvironment, shellEnvironment } = useTerminal();
  if (shellEnvironment?.state !== "failed") return null;

  return (
    <div className="flex shrink-0 items-start gap-2 border-b border-amber-500/20 bg-amber-500/10 px-3 py-2 text-xs text-amber-200">
      <DangerTriangleIcon className="mt-0.5 size-4 shrink-0" />
      <div className="min-w-0 flex-1">
        <p>
          Couldn't read your shell environment. Services start without what your shell config sets
          up.
        </p>
        {shellEnvironment.reason ? (
          <p className="mt-0.5 text-amber-200/70">{shellEnvironment.reason}</p>
        ) : null}
      </div>
      <Button
        type="button"
        size="xs"
        variant="ghost"
        className="shrink-0 text-amber-100 hover:bg-white/10 hover:text-white"
        onClick={() => void reload(reloadShellEnvironment)}
      >
        Reload shell environment
      </Button>
    </div>
  );
}

/** The terminal panel's menu. */
export function TerminalPanelMenu() {
  const { reloadShellEnvironment, shellEnvironment } = useTerminal();
  const resolving = shellEnvironment?.state === "resolving";

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            className="text-neutral-300 hover:bg-white/10 hover:text-white"
            aria-label="Terminal options"
          >
            <DotsIcon className="size-4" />
          </Button>
        }
      />
      <DropdownMenuContent align="end" className="w-fit">
        <DropdownMenuItem disabled={resolving} onClick={() => void reload(reloadShellEnvironment)}>
          <RefreshIcon className={resolving ? "animate-spin" : undefined} />
          Reload shell environment
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

async function reload(reloadShellEnvironment: () => Promise<void>) {
  try {
    await reloadShellEnvironment();
  } catch (error) {
    showError("Reload shell environment failed", error);
  }
}

function showError(title: string, error: unknown) {
  toast.error(title, {
    description: error instanceof Error ? error.message : String(error),
  });
}
