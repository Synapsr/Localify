import { useEffect, useState } from "react";

import type { RemoteInventoryDto, RemoteWatchEventDto } from "@/types/localify";

import { useWindowVisible } from "@/hooks/use-window-visible";
import { openRemoteStream } from "@/lib/remote-stream";

/** After the daemon closes a watch (it restarted), the app asks again this soon. */
const REOPEN_MS = 3000;

export type RemoteInventoryState =
  | { phase: "connecting"; inventory: RemoteInventoryDto | null }
  | { phase: "ready"; inventory: RemoteInventoryDto }
  | {
      phase: "lost";
      inventory: RemoteInventoryDto | null;
      code: string;
      message: string;
      retryIn: number;
    };

/**
 * Watches a paired machine's inventory while it is on screen and the window is visible
 * (docs/remote-machines.md, "Watches"): the daemon keeps one session to it, and asks again by
 * itself when the session is lost.
 */
export function useRemoteInventory(machineId: string): RemoteInventoryState {
  const visible = useWindowVisible();
  const [state, setState] = useState<RemoteInventoryState>({
    phase: "connecting",
    inventory: null,
  });

  useEffect(() => {
    setState({ phase: "connecting", inventory: null });
  }, [machineId]);

  useEffect(() => {
    if (!visible) return;
    let active = true;
    let close: (() => void) | null = null;
    let timer: number | undefined;
    const onEvent = (event: RemoteWatchEventDto) => {
      switch (event.event) {
        case "connecting":
          setState((current) =>
            current.phase === "ready"
              ? current
              : { phase: "connecting", inventory: current.inventory },
          );
          return;
        case "snapshot": {
          const { event: _, ...inventory } = event;
          setState({ phase: "ready", inventory });
          return;
        }
        case "lost":
          setState((current) => ({
            phase: "lost",
            inventory: current.inventory,
            code: event.code,
            message: event.message,
            retryIn: event.retryIn,
          }));
          return;
      }
    };
    const watch = async () => {
      try {
        close = await openRemoteStream<RemoteWatchEventDto>(
          { kind: "watch", machineId },
          {
            onData: onEvent,
            onEnd: () => {
              close = null;
              if (active) timer = window.setTimeout(() => void watch(), REOPEN_MS);
            },
          },
        );
        if (!active) close();
      } catch (error) {
        if (!active) return;
        setState((current) => ({
          phase: "lost",
          inventory: current.inventory,
          code: "unavailable",
          message: error instanceof Error ? error.message : String(error),
          retryIn: REOPEN_MS / 1000,
        }));
        timer = window.setTimeout(() => void watch(), REOPEN_MS);
      }
    };
    void watch();
    return () => {
      active = false;
      window.clearTimeout(timer);
      close?.();
    };
  }, [machineId, visible]);

  return state;
}
