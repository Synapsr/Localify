"use client";

import {
  createContext,
  type PropsWithChildren,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { toast } from "sonner";

import type {
  RemoteAccessEventDto,
  RemoteAccessWants,
  RemoteControlAction,
  RemoteControlEventDto,
  RemoteInventoryItemDto,
} from "@/types/localify";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { openRemoteStream } from "@/lib/remote-stream";
import { remoteErrorMessage, serviceKey } from "@/lib/util.remote";
import {
  type AccessView,
  accessEventView,
  type ControlPhase,
  controlLostText,
  controlProgressText,
  controlRequest,
  controlResultText,
  quickAction,
  type RelaunchConfirmation,
  relaunchConfirmation,
  relaunchStillConfirmed,
} from "@/lib/util.remote-controls";

/** After access is granted, how long a relaunch waits for the snapshot that allows it. */
const GRANTED_SNAPSHOT_MS = 15_000;

/** A control on its way to the target, one per service at a time, as the chip says it. */
interface ControlInFlight {
  text: string;
}

/** The access request this Mac has open with the machine on screen, if any. */
interface AccessRequestState {
  wants: RemoteAccessWants;
  view: AccessView;
}

interface RemoteControlsValue {
  machineName: string;
  /** The session's protocol; null while none is open. */
  proto: number | null;
  /** Why controls are off: the session is down, and nothing is queued. */
  down: string | null;
  inFlight: (item: RemoteInventoryItemDto) => ControlInFlight | null;
  run: (item: RemoteInventoryItemDto, action: RemoteControlAction) => void;
  access: AccessRequestState | null;
  ask: (wants: RemoteAccessWants) => void;
  cancelAsk: () => void;
  /** Forgets how the last request ended, once it was shown. */
  dismissAccess: () => void;
  /** Opens the relaunch confirmation for a legacy run. */
  relaunch: (item: RemoteInventoryItemDto) => void;
}

const RemoteControlsContext = createContext<RemoteControlsValue | null>(null);

interface RemoteControlsProviderProps {
  machineId: string;
  machineName: string;
  proto: number | null;
  down: string | null;
  appRunning: boolean;
  items: RemoteInventoryItemDto[];
}

/** A relaunch the person confirmed, waiting on access first. */
interface PendingRelaunch {
  key: string;
  confirmed: RelaunchConfirmation;
  wants: RemoteAccessWants;
  /** Access was granted: go ahead once the snapshot allows the restart. */
  granted: boolean;
}

/**
 * Start, Stop and Restart of the machine on screen, its access request and the relaunch of
 * legacy runs (docs/remote-machines.md, "Quick actions", "State machine", the viewer side).
 */
export function RemoteControlsProvider({
  machineId,
  machineName,
  proto,
  down,
  appRunning,
  items,
  children,
}: PropsWithChildren<RemoteControlsProviderProps>) {
  const [controls, setControls] = useState<Record<string, ControlInFlight>>({});
  // Set at the click, before React renders: a second click finds it.
  const busy = useRef(new Set<string>());
  const [access, setAccess] = useState<AccessRequestState | null>(null);
  const closeAccess = useRef<(() => void) | null>(null);
  // Counts asks: events of an ask replaced or cancelled since are dropped.
  const askSeq = useRef(0);
  const [confirming, setConfirming] = useState<RemoteInventoryItemDto | null>(null);
  const [pending, setPending] = useState<PendingRelaunch | null>(null);
  const live = useRef({ machineName, appRunning });
  live.current = { machineName, appRunning };

  // A machine left on screen keeps nothing open with it.
  useEffect(
    () => () => {
      askSeq.current += 1;
      closeAccess.current?.();
      closeAccess.current = null;
    },
    [machineId],
  );

  const run = useCallback(
    (
      item: RemoteInventoryItemDto,
      action: RemoteControlAction,
      relaunch: RelaunchConfirmation | null = null,
    ) => {
      const key = serviceKey(item);
      if (busy.current.has(key)) return;
      busy.current.add(key);
      const service = item.service.name;
      const toastId = `remote-control:${machineId}:${key}`;
      const progress = (phase: ControlPhase) => {
        const { machineName: name, appRunning: app } = live.current;
        const text = controlProgressText(action, phase, name, app);
        setControls((current) => ({ ...current, [key]: { text } }));
        toast.loading(text, { id: toastId });
      };
      const settle = (ok: boolean, text: string) => {
        busy.current.delete(key);
        setControls(({ [key]: _, ...rest }) => rest);
        if (ok) toast.success(text, { id: toastId });
        else toast.error(text, { id: toastId });
      };
      progress("sent");
      let ended = false;
      const onEvent = (event: RemoteControlEventDto) => {
        const name = live.current.machineName;
        switch (event.event) {
          case "sent":
            return;
          case "progress":
            progress(event.phase);
            return;
          case "unknown":
            progress("unknown");
            return;
          case "result": {
            ended = true;
            const view = controlResultText(event, action, service, name);
            settle(view.ok, view.text);
            return;
          }
          case "lost":
            ended = true;
            settle(false, controlLostText(name));
            return;
        }
      };
      void openRemoteStream<RemoteControlEventDto>(
        { kind: "control", machineId, ...controlRequest(item, action, relaunch) },
        {
          onData: onEvent,
          onEnd: (error) => {
            if (!ended) settle(false, error ?? controlLostText(live.current.machineName));
          },
        },
      ).catch((error: unknown) =>
        settle(false, error instanceof Error ? error.message : String(error)),
      );
    },
    [machineId],
  );

  /**
   * Asks the target for access. A card shows the request as it goes; a relaunch confirmed in
   * its dialog has no card, so a toast shows it, with Cancel.
   */
  const ask = useCallback(
    (wants: RemoteAccessWants, inToast = false) => {
      closeAccess.current?.();
      closeAccess.current = null;
      const seq = ++askSeq.current;
      const isLatest = () => seq === askSeq.current;
      const toastId = `remote-access:${machineId}`;
      const show = (view: AccessView) => {
        setAccess({ wants, view });
        if (view.step === "waiting") {
          if (inToast) {
            toast.loading(view.text, {
              id: toastId,
              action: { label: "Cancel", onClick: () => cancelRef.current() },
            });
          }
        } else if (view.ok) toast.success(view.text, { id: toastId });
        else if (inToast) toast.error(view.text, { id: toastId });
      };
      show(accessEventView({ event: "waiting" }, wants, live.current.machineName));
      let ended = false;
      void openRemoteStream<RemoteAccessEventDto>(
        { kind: "access", machineId, wants },
        {
          onData: (event) => {
            if (!isLatest()) return;
            const view = accessEventView(event, wants, live.current.machineName);
            if (view.step === "ended") {
              ended = true;
              closeAccess.current = null;
              setPending((current) =>
                current && event.event === "result" && event.outcome === "granted"
                  ? { ...current, granted: true }
                  : null,
              );
            }
            show(view);
          },
          onEnd: (error) => {
            if (!isLatest()) return;
            closeAccess.current = null;
            if (ended) return;
            setPending(null);
            show(
              error
                ? { step: "ended", ok: false, text: error }
                : accessEventView(
                    { event: "lost", code: "closed", message: "" },
                    wants,
                    live.current.machineName,
                  ),
            );
          },
        },
      )
        .then((close) => {
          if (ended || !isLatest()) close();
          else closeAccess.current = close;
        })
        .catch((error: unknown) => {
          if (!isLatest()) return;
          setPending(null);
          show({
            step: "ended",
            ok: false,
            text: error instanceof Error ? error.message : String(error),
          });
        });
    },
    [machineId],
  );

  // Closing the stream sends `access.cancel`.
  const cancelAsk = useCallback(() => {
    askSeq.current += 1;
    closeAccess.current?.();
    closeAccess.current = null;
    setAccess(null);
    setPending(null);
    toast.dismiss(`remote-access:${machineId}`);
  }, [machineId]);
  const cancelRef = useRef(cancelAsk);
  cancelRef.current = cancelAsk;

  const dismissAccess = useCallback(
    () => setAccess((current) => (current?.view.step === "ended" ? null : current)),
    [],
  );

  const confirmRelaunch = () => {
    const item = confirming;
    setConfirming(null);
    const confirmed = item ? relaunchConfirmation(item) : null;
    if (!item || !confirmed) return;
    if (down) {
      toast.error(down);
      return;
    }
    const next = quickAction(item, "restart", proto, machineName);
    if (next.kind !== "relaunch") {
      toast.error(next.kind === "message" ? next.text : `${machineName} changed in the meantime.`);
      return;
    }
    if (next.access) {
      setPending({ key: serviceKey(item), confirmed, wants: next.access, granted: false });
      ask(next.access, true);
      return;
    }
    run(item, "restart", confirmed);
  };

  // Once access is granted, the next snapshot carries the controls it allows.
  useEffect(() => {
    // Nothing is sent while the session is down; the timeout below gives up.
    if (!pending?.granted || down) return;
    const item = items.find((candidate) => serviceKey(candidate) === pending.key);
    if (!item) {
      setPending(null);
      toast.error(`This service no longer exists on ${machineName}.`);
      return;
    }
    const next = quickAction(item, "restart", proto, machineName);
    if (next.kind === "relaunch" && next.access === null) {
      setPending(null);
      if (relaunchStillConfirmed(pending.confirmed, item)) run(item, "restart", pending.confirmed);
      // It changed since the person confirmed: ask again.
      else if (relaunchConfirmation(item)) setConfirming(item);
      else toast.error(`${machineName} changed in the meantime.`);
      return;
    }
    // A snapshot from before the grant: wait for the next one.
    if (next.kind === "relaunch" && next.access === pending.wants) return;
    setPending(null);
    toast.error(
      next.kind === "message"
        ? next.text
        : remoteErrorMessage(
            item.controls.restart.code ?? "",
            machineName,
            `${machineName} changed in the meantime.`,
          ),
    );
  }, [items, pending, proto, machineName, run, down]);

  useEffect(() => {
    if (!pending?.granted) return;
    const timer = window.setTimeout(() => {
      setPending(null);
      toast.error(`${machineName} allowed it, but its new state didn't arrive. Try again.`);
    }, GRANTED_SNAPSHOT_MS);
    return () => window.clearTimeout(timer);
  }, [pending?.granted, machineName]);

  const value = useMemo<RemoteControlsValue>(
    () => ({
      machineName,
      proto,
      down,
      inFlight: (item) => controls[serviceKey(item)] ?? null,
      run: (item, action) => run(item, action),
      access,
      // Asking from a card is not a relaunch's.
      ask: (wants) => {
        setPending(null);
        ask(wants);
      },
      cancelAsk,
      dismissAccess,
      relaunch: setConfirming,
    }),
    [machineName, proto, down, controls, run, access, ask, cancelAsk, dismissAccess],
  );

  return (
    <RemoteControlsContext.Provider value={value}>
      {children}
      <RelaunchDialog
        item={confirming}
        machineName={machineName}
        onCancel={() => setConfirming(null)}
        onConfirm={confirmRelaunch}
      />
    </RemoteControlsContext.Provider>
  );
}

export function useRemoteControls(): RemoteControlsValue {
  const context = useContext(RemoteControlsContext);
  if (!context) throw new Error("useRemoteControls must be used inside RemoteControlsProvider");
  return context;
}

/** The contract's confirmation before a legacy run is relaunched. */
function RelaunchDialog({
  item,
  machineName,
  onCancel,
  onConfirm,
}: {
  item: RemoteInventoryItemDto | null;
  machineName: string;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <Dialog open={item !== null} onOpenChange={(open) => (open ? undefined : onCancel())}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Relaunch on {machineName}</DialogTitle>
          <DialogDescription>
            {item?.service.name} started before this version of Localify on {machineName}. Relaunch
            it so its output and controls work here? It restarts with its configured command.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={onCancel}>
            Cancel
          </Button>
          <Button variant="primary" onClick={onConfirm}>
            Relaunch on {machineName}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
