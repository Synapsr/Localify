"use client";

import { useQuery, useQueryClient } from "@tanstack/react-query";
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

import type { RemoteEventDto, RemoteMachineDto, RemotePairEventDto } from "@/types/localify";

import { RemotePairingDialogs } from "@/components/app/remote/remote-pairing-dialogs";
import { remoteQueries } from "@/lib/queries";
import { openRemoteStream } from "@/lib/remote-stream";
import { tauriClient } from "@/lib/tauri-client";
import { pairingCancelledMessage } from "@/lib/util.remote";

/** How long the app waits before it listens to the daemon again. */
const EVENTS_RETRY_MS = 5000;

/** A pairing this Mac started, as the viewer. */
export type OutgoingPairing =
  | { step: "asking"; machine: RemoteMachineDto }
  | { step: "code"; machine: RemoteMachineDto; pairingId: string; code: string; answered: boolean }
  | { step: "paired"; machine: RemoteMachineDto }
  | { step: "cancelled"; machine: RemoteMachineDto; message: string };

/** A pairing another machine asked for, with this Mac as the target. */
export type IncomingPairing =
  | {
      step: "code";
      pairingId: string;
      machineId: string;
      name: string;
      code: string;
      answered: boolean;
    }
  | { step: "paired"; pairingId: string; name: string }
  | { step: "cancelled"; pairingId: string; name: string };

interface RemoteContextValue {
  /** The machine on screen; null for this Mac. */
  selectedMachineId: string | null;
  selectMachine: (machineId: string | null) => void;
  startPairing: (machine: RemoteMachineDto) => void;
  cancelOutgoing: () => void;
  dismissOutgoing: () => void;
  confirmOutgoing: () => Promise<void>;
  outgoing: OutgoingPairing | null;
  incoming: IncomingPairing | null;
  answerIncoming: (accept: boolean) => Promise<void>;
  dismissIncoming: () => void;
  /** A remote service's output covers the bottom of the window, as the terminal does. */
  outputOpen: boolean;
  setOutputOpen: (open: boolean) => void;
}

const RemoteContext = createContext<RemoteContextValue | null>(null);

export function RemoteProvider({ children }: PropsWithChildren) {
  const queryClient = useQueryClient();
  const machines = useQuery(remoteQueries.machines());
  const [selectedMachineId, setSelectedMachineId] = useState<string | null>(null);
  const [outgoing, setOutgoing] = useState<OutgoingPairing | null>(null);
  const [incoming, setIncoming] = useState<IncomingPairing | null>(null);
  const [outputOpen, setOutputOpen] = useState(false);
  const closeOutgoing = useRef<(() => void) | null>(null);
  const incomingRef = useRef(incoming);
  incomingRef.current = incoming;

  const refreshMachines = useCallback(
    () => void queryClient.invalidateQueries({ queryKey: remoteQueries.all() }),
    [queryClient],
  );

  // While the app runs, a person can confirm a pairing on this Mac.
  useEffect(() => {
    let active = true;
    let close: (() => void) | null = null;
    let timer: number | undefined;
    const onEvent = (event: RemoteEventDto) => {
      switch (event.event) {
        case "pair_request":
          setIncoming({ step: "code", ...event, answered: false });
          return;
        case "paired":
          setIncoming((current) =>
            current && current.pairingId === event.pairingId
              ? { step: "paired", pairingId: event.pairingId, name: event.name }
              : current,
          );
          refreshMachines();
          return;
        case "pair_cancelled":
          setIncoming((current) =>
            current && current.pairingId === event.pairingId && current.step === "code"
              ? { step: "cancelled", pairingId: event.pairingId, name: current.name }
              : current,
          );
          return;
        case "presence":
        case "revoked":
        case "unpaired":
          refreshMachines();
          return;
        case "subscribed":
          return;
      }
    };
    const listen = async () => {
      try {
        close = await openRemoteStream<RemoteEventDto>(
          { kind: "events" },
          {
            onData: onEvent,
            onEnd: () => {
              close = null;
              if (active) timer = window.setTimeout(() => void listen(), EVENTS_RETRY_MS);
            },
          },
        );
        if (!active) close();
      } catch {
        // The daemon is not running, or predates remote machines.
        if (active) timer = window.setTimeout(() => void listen(), EVENTS_RETRY_MS);
      }
    };
    void listen();
    return () => {
      active = false;
      window.clearTimeout(timer);
      close?.();
    };
  }, [refreshMachines]);

  // A machine that left the account, or was unpaired, is no longer on screen.
  useEffect(() => {
    if (!selectedMachineId || !machines.data) return;
    const selected = machines.data.find((machine) => machine.id === selectedMachineId);
    if (!selected?.paired) setSelectedMachineId(null);
  }, [machines.data, selectedMachineId]);

  const startPairing = useCallback(
    (machine: RemoteMachineDto) => {
      closeOutgoing.current?.();
      setOutgoing({ step: "asking", machine });
      const onEvent = (event: RemotePairEventDto) => {
        switch (event.event) {
          case "code":
            setOutgoing({
              step: "code",
              machine,
              pairingId: event.pairingId,
              code: event.code,
              answered: false,
            });
            return;
          case "paired":
            setOutgoing({ step: "paired", machine });
            refreshMachines();
            return;
          case "cancelled":
            setOutgoing({
              step: "cancelled",
              machine,
              message: pairingCancelledMessage(event.reason, event.message, machine.name),
            });
            return;
        }
      };
      void openRemoteStream<RemotePairEventDto>(
        { kind: "pair", machineId: machine.id },
        {
          onData: onEvent,
          onEnd: (error) => {
            closeOutgoing.current = null;
            setOutgoing((current) =>
              current && (current.step === "asking" || current.step === "code")
                ? {
                    step: "cancelled",
                    machine,
                    message: error ?? "Pairing cancelled. Nothing was saved.",
                  }
                : current,
            );
          },
        },
      )
        .then((close) => {
          closeOutgoing.current = close;
        })
        .catch((error: unknown) =>
          setOutgoing({
            step: "cancelled",
            machine,
            message: error instanceof Error ? error.message : String(error),
          }),
        );
    },
    [refreshMachines],
  );

  const confirmOutgoing = useCallback(async () => {
    if (outgoing?.step !== "code") return;
    setOutgoing({ ...outgoing, answered: true });
    await tauriClient.answerRemotePairing({ pairingId: outgoing.pairingId, accept: true });
  }, [outgoing]);

  // Closing the stream cancels the pairing on both sides.
  const cancelOutgoing = useCallback(() => {
    closeOutgoing.current?.();
    closeOutgoing.current = null;
    setOutgoing(null);
  }, []);

  const dismissOutgoing = useCallback(() => setOutgoing(null), []);

  const answerIncoming = useCallback(async (accept: boolean) => {
    const current = incomingRef.current;
    if (current?.step !== "code") return;
    if (accept) setIncoming({ ...current, answered: true });
    else setIncoming(null);
    await tauriClient.answerRemotePairing({ pairingId: current.pairingId, accept });
  }, []);

  const dismissIncoming = useCallback(() => setIncoming(null), []);

  const value = useMemo<RemoteContextValue>(
    () => ({
      selectedMachineId,
      selectMachine: setSelectedMachineId,
      startPairing,
      cancelOutgoing,
      dismissOutgoing,
      confirmOutgoing,
      outgoing,
      incoming,
      answerIncoming,
      dismissIncoming,
      outputOpen,
      setOutputOpen,
    }),
    [
      selectedMachineId,
      startPairing,
      cancelOutgoing,
      dismissOutgoing,
      confirmOutgoing,
      outgoing,
      incoming,
      answerIncoming,
      dismissIncoming,
      outputOpen,
    ],
  );

  return (
    <RemoteContext.Provider value={value}>
      {children}
      <RemotePairingDialogs />
    </RemoteContext.Provider>
  );
}

export function useRemote(): RemoteContextValue {
  const context = useContext(RemoteContext);
  if (!context) throw new Error("useRemote must be used inside RemoteProvider");
  return context;
}
