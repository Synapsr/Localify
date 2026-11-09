"use client";

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
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
  ShellEnvironmentDto,
  TerminalExitPayload,
  TerminalLaunchServiceResultDto,
  TerminalOpenedPayload,
  TerminalOutputPayload,
  TerminalSessionDto,
  TerminalSessionTargetDto,
  TerminalTargetRequest,
} from "@/types/localify";

import { setDeepLinkLaunchHandler } from "@/lib/deep-link";
import { bootstrapQueries } from "@/lib/queries";
import { tauriClient } from "@/lib/tauri-client";
import {
  laterSession,
  mergeSessionList,
  reachableServiceSessionIds,
  sortTerminalSessions,
  type TerminalSessionState,
} from "@/lib/util.terminal-sessions";
import { useSharedProjectOrder } from "@/providers/project-order-provider";

const DEFAULT_COLS = 120;
const DEFAULT_ROWS = 32;
const DEFAULT_PANEL_HEIGHT = 360;
const MIN_PANEL_HEIGHT = 220;
const MAX_PANEL_HEIGHT_RATIO = 0.82;
const MAX_BUFFER_BYTES = 1_000_000;
const TERMINAL_HEIGHT_STORAGE_KEY = "localify.terminal.height";
const STARTED_BY_MS = 5_000;

type OutputBySessionId = Record<string, Uint8Array[]>;

interface TerminalContextValue {
  activeSession: TerminalSessionDto | null;
  activeSessionId: string | null;
  closePanel: () => void;
  closeSession: (sessionId?: string) => Promise<void>;
  findSessionForTarget: (target: TerminalTargetRequest) => TerminalSessionDto | null;
  /** Bumped when the active session should take keyboard focus again. */
  focusRequestId: number;
  hasOwnedServiceSession: (target: TerminalTargetRequest) => boolean;
  isOpen: boolean;
  launchService: (
    target: TerminalTargetRequest,
    profile?: string | null,
  ) => Promise<TerminalSessionDto>;
  newTabFromActiveTarget: () => Promise<void>;
  openSelectedTarget: () => Promise<TerminalSessionDto | null>;
  openPanel: () => void;
  openTerminal: (target: TerminalTargetRequest) => Promise<TerminalSessionDto>;
  outputBySessionId: OutputBySessionId;
  panelHeight: number;
  reloadShellEnvironment: () => Promise<void>;
  resizeSession: (sessionId: string, cols: number, rows: number) => Promise<void>;
  /** Stops the tab's run, then starts the service again in a fresh run PTY in the same tab. */
  restartSession: (sessionId: string) => Promise<void>;
  selectEnvironment: (projectId: string, environmentSlug: string) => void;
  selectNextEnvironment: () => void;
  selectNextSession: () => void;
  selectPreviousEnvironment: () => void;
  selectPreviousSession: () => void;
  sessionStates: Map<string, TerminalSessionState>;
  /** The paired machine that just started a tab, by session id, for a few seconds. */
  startedBy: Record<string, string>;
  /** Whether the environment services start with was read from the user's shell. */
  shellEnvironment: ShellEnvironmentDto | null;
  /** Stops a service tab's run through its session, never with a Ctrl-C. */
  stopSession: (sessionId: string) => Promise<void>;
  /** Every session, in tab order. */
  sessions: TerminalSessionDto[];
  setActiveSessionId: (sessionId: string) => void;
  setPanelHeight: (height: number) => void;
  selectedTarget: TerminalTargetRequest | null;
  setSelectedTarget: (target: TerminalTargetRequest | null) => void;
  togglePanel: () => void;
  writeToSession: (sessionId: string, data: string) => Promise<void>;
}

const TerminalContext = createContext<TerminalContextValue | null>(null);

export const TerminalProvider = ({ children }: PropsWithChildren) => {
  const queryClient = useQueryClient();
  const { data: bootstrap } = useQuery(bootstrapQueries.read());
  const { order: projectOrder } = useSharedProjectOrder();
  const [unsortedSessions, setSessions] = useState<TerminalSessionDto[]>([]);
  const [outputBySessionId, setOutputBySessionId] = useState<OutputBySessionId>({});
  const [activeSessionId, setActiveSessionIdState] = useState<string | null>(null);
  const [isOpen, setIsOpen] = useState(false);
  const [panelHeight, setPanelHeightState] = useState(loadStoredPanelHeight);
  const [selectedTarget, setSelectedTargetState] = useState<TerminalTargetRequest | null>(null);
  const [focusRequestId, setFocusRequestId] = useState(0);
  const [shellEnvironment, setShellEnvironment] = useState<ShellEnvironmentDto | null>(null);
  const [startedBy, setStartedBy] = useState<Record<string, string>>({});
  const projectEnvironments = bootstrap?.projectEnvironments;
  const sessions = useMemo(
    () => sortTerminalSessions(unsortedSessions, projectOrder, projectEnvironments ?? []),
    [projectEnvironments, projectOrder, unsortedSessions],
  );
  const sessionsRef = useRef(sessions);
  const lastSessionByEnvironmentRef = useRef(new Map<string, string>());
  // The newest known state of every session, events and answers alike.
  const latestSessionsRef = useRef(new Map<string, TerminalSessionDto>());
  const reconcile = useCallback((incoming: TerminalSessionDto) => {
    const next = laterSession(latestSessionsRef.current.get(incoming.id), incoming);
    latestSessionsRef.current.set(incoming.id, next);
    return next;
  }, []);
  // Sessions replaced or closed here, so a list of sessions asked for earlier never brings them back.
  const goneSessionIdsRef = useRef(new Set<string>());
  const [stoppedSessionIds, setStoppedSessionIds] = useState<ReadonlySet<string>>(new Set());
  const previousReachableIdsRef = useRef<ReadonlySet<string>>(new Set());
  const expectedStopIdsRef = useRef(new Set<string>());
  const domains = bootstrap?.domains;
  const reachableIds = useMemo(
    () => reachableServiceSessionIds(sessions, projectEnvironments ?? [], domains ?? []),
    [domains, projectEnvironments, sessions],
  );

  useEffect(() => {
    sessionsRef.current = sessions;
  }, [sessions]);

  // Puts a new run in its tab's place: the one it took over, or a new one.
  const insertRun = useCallback(
    (result: TerminalLaunchServiceResultDto) => {
      const { replaced } = result;
      if (replaced) goneSessionIdsRef.current.add(replaced);
      const session = reconcile(result.session);
      setSessions((current) => {
        const index = replaced ? current.findIndex((candidate) => candidate.id === replaced) : -1;
        const others = current.filter(
          (candidate) => candidate.id !== session.id && candidate.id !== replaced,
        );
        if (index === -1) {
          return current.some((candidate) => candidate.id === session.id)
            ? current.map((candidate) => (candidate.id === session.id ? session : candidate))
            : [...others, session];
        }
        return [...others.slice(0, index), session, ...others.slice(index)];
      });
      if (replaced) {
        setOutputBySessionId((current) => {
          const next = { ...current };
          delete next[replaced];
          return next;
        });
      }
      return session;
    },
    [reconcile],
  );

  // Every event is listened to before the sessions are listed, and the list is merged into what
  // the events brought meanwhile: a tab the app bridge opens during startup is in one or both, and
  // never lost or overwritten by an older list.
  useEffect(() => {
    const unlisteners: (() => void)[] = [];
    let cancelled = false;
    const subscribe = async <T,>(name: string, handler: (payload: T) => void) => {
      const unlisten = await listen<T>(name, (event) => {
        if (!cancelled) handler(event.payload);
      });
      if (cancelled) unlisten();
      else unlisteners.push(unlisten);
    };

    void tauriClient
      .shellEnvironmentStatus()
      .then((status) => {
        if (!cancelled) setShellEnvironment(status);
      })
      .catch(() => {});

    const subscribed = Promise.allSettled([
      // A service tab's run ended and its output was finalized. Kept even before the answer
      // that opened the tab arrives, so that answer cannot show it running again.
      subscribe<TerminalSessionDto>("terminal-session", (payload) => {
        const ended = reconcile(payload);
        setSessions((current) =>
          current.map((session) => (session.id === ended.id ? ended : session)),
        );
        void queryClient.invalidateQueries({ queryKey: bootstrapQueries.all() });
      }),
      // A launch handed to the app, by `localify dev --in-app` or a paired machine: its tab
      // joins the others without taking focus, unless it took over the tab in view.
      subscribe<TerminalOpenedPayload>("terminal-session-opened", (payload) => {
        const { replaced, startedBy: name } = payload;
        const session = insertRun(payload);
        setActiveSessionIdState((current) =>
          current === null || current === replaced ? session.id : current,
        );
        if (name) {
          setStartedBy((current) => ({ ...current, [session.id]: name }));
          window.setTimeout(() => {
            setStartedBy((current) => {
              const next = { ...current };
              delete next[session.id];
              return next;
            });
          }, STARTED_BY_MS);
        }
        void queryClient.invalidateQueries({ queryKey: bootstrapQueries.all() });
      }),
      subscribe<ShellEnvironmentDto>("shell-environment", setShellEnvironment),
      subscribe<TerminalOutputPayload>("terminal-output", (payload) => {
        const chunk = new Uint8Array(payload.bytes);
        setOutputBySessionId((current) => appendOutputChunk(current, payload.sessionId, chunk));
      }),
      subscribe<TerminalExitPayload>("terminal-exit", (payload) => {
        setSessions((current) =>
          current.map((session) =>
            session.id === payload.sessionId
              ? reconcile({ ...session, status: "exited" })
              : session,
          ),
        );
        void queryClient.invalidateQueries({ queryKey: bootstrapQueries.all() });
      }),
    ]);

    void subscribed
      .then(() => (cancelled ? null : tauriClient.terminalListSessions()))
      .then((listed) => {
        if (cancelled || !listed) return;
        const initialSession = listed[0] ?? null;
        const known = listed.map(reconcile);
        setSessions((current) => mergeSessionList(current, known, goneSessionIdsRef.current));
        setActiveSessionIdState((current) => current ?? initialSession?.id ?? null);
        setSelectedTargetState(
          (current) =>
            current ?? (initialSession ? toTerminalTargetRequest(initialSession.target) : null),
        );
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      for (const unlisten of unlisteners) unlisten();
    };
  }, [insertRun, queryClient, reconcile]);

  // Bootstrap polling is what sees a service stop. A stop the user just asked for, or watches
  // in the active tab, needs no warning, and looking at the tab clears it.
  useEffect(() => {
    const previousReachableIds = previousReachableIdsRef.current;
    previousReachableIdsRef.current = reachableIds;

    setStoppedSessionIds((current) => {
      const next = new Set(current);
      next.delete(activeSessionId ?? "");
      for (const id of reachableIds) {
        next.delete(id);
      }
      for (const id of previousReachableIds) {
        if (reachableIds.has(id) || !sessionsRef.current.some((session) => session.id === id)) {
          continue;
        }
        if (!expectedStopIdsRef.current.delete(id) && id !== activeSessionId) {
          next.add(id);
        }
      }
      return next.size === current.size && [...next].every((id) => current.has(id))
        ? current
        : next;
    });
  }, [activeSessionId, reachableIds]);

  const setSelectedTarget = useCallback((target: TerminalTargetRequest | null) => {
    setSelectedTargetState(target);
    if (!target) {
      return;
    }

    const matchingSession = sessionsRef.current.find((session) =>
      targetEnvironmentMatches(toTerminalTargetRequest(session.target), target),
    );
    if (matchingSession) {
      setActiveSessionIdState(matchingSession.id);
    }
  }, []);

  const setPanelHeight = useCallback((height: number) => {
    const clamped = clampPanelHeight(height);
    setPanelHeightState(clamped);
    window.localStorage.setItem(TERMINAL_HEIGHT_STORAGE_KEY, String(clamped));
  }, []);

  const setActiveSessionId = useCallback((sessionId: string) => {
    const session = sessionsRef.current.find((candidate) => candidate.id === sessionId);
    if (session) {
      setSelectedTargetState(toTerminalTargetRequest(session.target));
    }
    setActiveSessionIdState(sessionId);
    setIsOpen(true);
  }, []);

  const upsertSession = useCallback(
    (incoming: TerminalSessionDto) => {
      const session = reconcile(incoming);
      setSessions((current) => {
        const index = current.findIndex((candidate) => candidate.id === session.id);
        if (index === -1) {
          return [...current, session];
        }

        const next = [...current];
        next[index] = session;
        return next;
      });
      setActiveSessionIdState(session.id);
      setSelectedTargetState(toTerminalTargetRequest(session.target));
      setIsOpen(true);
    },
    [reconcile],
  );

  // A service target's running tab is its run; any other target's is a shell.
  const findSessionForTarget = useCallback((target: TerminalTargetRequest) => {
    return (
      sessionsRef.current.find(
        (session) =>
          session.status === "running" &&
          Boolean(session.run) === (target.kind === "service") &&
          terminalTargetMatches(session.target, target),
      ) ?? null
    );
  }, []);

  // Always a shell, also for a service: `Open a terminal here` next to its tab.
  const findShellForTarget = useCallback((target: TerminalTargetRequest) => {
    return (
      sessionsRef.current.find(
        (session) =>
          session.status === "running" &&
          !session.run &&
          terminalTargetMatches(session.target, target),
      ) ?? null
    );
  }, []);

  // A new run that took over an ended tab, or a restart, keeps the tab's place.
  const placeRun = useCallback(
    (result: TerminalLaunchServiceResultDto) => {
      const session = insertRun(result);
      setActiveSessionIdState(session.id);
      setSelectedTargetState(toTerminalTargetRequest(session.target));
      setIsOpen(true);
    },
    [insertRun],
  );

  const openTerminal = useCallback(
    async (target: TerminalTargetRequest) => {
      const existing = findShellForTarget(target);
      if (existing) {
        setActiveSessionId(existing.id);
        return existing;
      }

      const session = await tauriClient.terminalCreateSession({
        target,
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
      });
      upsertSession(session);
      return session;
    },
    [findShellForTarget, setActiveSessionId, upsertSession],
  );

  // `profile` is never remembered: every launch starts from the default profile
  // unless the caller picks one for this launch only.
  const launchService = useCallback(
    async (target: TerminalTargetRequest, profile: string | null = null) => {
      const result = await tauriClient.terminalLaunchService({
        target,
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
        profile,
      });
      placeRun(result);
      // `localify dev` may prompt (for example to confirm a profile),
      // so focus the terminal even when the launch returned the active session.
      setFocusRequestId((current) => current + 1);
      await queryClient.invalidateQueries({ queryKey: bootstrapQueries.all() });
      return result.session;
    },
    [placeRun, queryClient],
  );

  const restartSession = useCallback(
    async (sessionId: string) => {
      expectedStopIdsRef.current.add(sessionId);
      const result = await tauriClient.terminalRestartService({
        sessionId,
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
      });
      placeRun(result);
      setFocusRequestId((current) => current + 1);
      await queryClient.invalidateQueries({ queryKey: bootstrapQueries.all() });
    },
    [placeRun, queryClient],
  );

  const reloadShellEnvironment = useCallback(async () => {
    setShellEnvironment((current) => (current ? { ...current, state: "resolving" } : current));
    setShellEnvironment(await tauriClient.reloadShellEnvironment());
  }, []);

  // The `localify://action/launch-service` link starts a service through the same path as
  // the launch button. `localify dev --in-app` uses the app bridge instead.
  useEffect(() => {
    setDeepLinkLaunchHandler(async (launch) => {
      setIsOpen(true);
      try {
        await launchService(
          {
            kind: "service",
            projectId: launch.projectId,
            environmentSlug: launch.environmentSlug,
            serviceId: launch.serviceId,
          },
          launch.profile,
        );
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        toast.error(`Could not start ${launch.serviceId}`, { description: message });
      }
    });
    return () => setDeepLinkLaunchHandler(null);
  }, [launchService]);

  const openSelectedTarget = useCallback(async () => {
    if (!selectedTarget) {
      setIsOpen(true);
      return null;
    }

    return openTerminal(selectedTarget);
  }, [openTerminal, selectedTarget]);

  const writeToSession = useCallback(async (sessionId: string, data: string) => {
    await tauriClient.terminalWrite({ sessionId, data });
  }, []);

  const resizeSession = useCallback(async (sessionId: string, cols: number, rows: number) => {
    await tauriClient.terminalResize({ sessionId, cols, rows });
  }, []);

  const stopSession = useCallback(
    async (sessionId: string) => {
      if (previousReachableIdsRef.current.has(sessionId)) {
        expectedStopIdsRef.current.add(sessionId);
      }
      await tauriClient.terminalStopService({ sessionId });
      await queryClient.invalidateQueries({ queryKey: bootstrapQueries.all() });
    },
    [queryClient],
  );

  const closeSession = useCallback(
    async (sessionId = activeSessionId ?? undefined) => {
      if (!sessionId) return;
      const currentSessions = sessionsRef.current;
      const session = currentSessions.find((candidate) => candidate.id === sessionId);
      if (!session) return;

      if (
        session.run &&
        session.status === "running" &&
        !window.confirm("Close this service tab and stop the service?")
      ) {
        return;
      }

      await tauriClient.terminalClose({ sessionId });
      goneSessionIdsRef.current.add(sessionId);
      // From the sessions as they are now: a tab the bridge opened while the
      // close was awaited stays.
      const latestSessions = sessionsRef.current;
      const closedIndex = latestSessions.findIndex((candidate) => candidate.id === sessionId);
      const next = latestSessions.filter((candidate) => candidate.id !== sessionId);
      setSessions((current) => current.filter((candidate) => candidate.id !== sessionId));
      setActiveSessionIdState((currentActive) => {
        if (currentActive !== sessionId) return currentActive;
        const neighbors = [next[closedIndex], next[closedIndex - 1]].filter(Boolean);
        const nextActive =
          neighbors.find((candidate) =>
            sessionEnvironmentMatches(candidate.target, session.target),
          ) ??
          neighbors[0] ??
          null;
        setSelectedTargetState(nextActive ? toTerminalTargetRequest(nextActive.target) : null);
        return nextActive?.id ?? null;
      });
      if (next.length === 0) {
        setIsOpen(false);
      }
      setOutputBySessionId((current) => {
        const next = { ...current };
        delete next[sessionId];
        return next;
      });
      await queryClient.invalidateQueries({ queryKey: bootstrapQueries.all() });
    },
    [activeSessionId, queryClient],
  );

  const selectNextSession = useCallback(() => {
    const nextSessionId = adjacentSessionId(sessionsRef.current, activeSessionId, 1);
    if (nextSessionId) {
      setActiveSessionId(nextSessionId);
    }
    setIsOpen(true);
  }, [activeSessionId, setActiveSessionId]);

  const selectPreviousSession = useCallback(() => {
    const nextSessionId = adjacentSessionId(sessionsRef.current, activeSessionId, -1);
    if (nextSessionId) {
      setActiveSessionId(nextSessionId);
    }
    setIsOpen(true);
  }, [activeSessionId, setActiveSessionId]);

  const activeSession = useMemo(
    () => sessions.find((session) => session.id === activeSessionId) ?? null,
    [activeSessionId, sessions],
  );

  useEffect(() => {
    if (activeSession) {
      lastSessionByEnvironmentRef.current.set(
        environmentKey(activeSession.target),
        activeSession.id,
      );
    }
  }, [activeSession]);

  // Coming back to a checkout reopens the tab left open there.
  const selectEnvironment = useCallback(
    (projectId: string, environmentSlug: string) => {
      const key = environmentKey({ projectId, environmentSlug });
      const candidates = sessionsRef.current.filter(
        (session) => environmentKey(session.target) === key,
      );
      const lastSessionId = lastSessionByEnvironmentRef.current.get(key);
      const session =
        candidates.find((candidate) => candidate.id === lastSessionId) ?? candidates[0];
      if (session) {
        setActiveSessionId(session.id);
      }
    },
    [setActiveSessionId],
  );

  const selectAdjacentEnvironment = useCallback(
    (offset: 1 | -1) => {
      const firstSessions = sessionsRef.current.filter(
        (session, index, all) =>
          all.findIndex(
            (candidate) => environmentKey(candidate.target) === environmentKey(session.target),
          ) === index,
      );
      if (firstSessions.length === 0) return;

      const currentIndex = activeSession
        ? firstSessions.findIndex(
            (session) => environmentKey(session.target) === environmentKey(activeSession.target),
          )
        : -1;
      const next =
        firstSessions[(currentIndex + offset + firstSessions.length) % firstSessions.length];
      selectEnvironment(next.target.projectId, next.target.environmentSlug);
    },
    [activeSession, selectEnvironment],
  );

  const sessionStates = useMemo(() => {
    const states = new Map<string, TerminalSessionState>();
    for (const session of sessions) {
      const live = session.status === "running" && (!session.run || reachableIds.has(session.id));
      states.set(session.id, stoppedSessionIds.has(session.id) ? "failed" : live ? "live" : "idle");
    }
    return states;
  }, [reachableIds, sessions, stoppedSessionIds]);

  const newTabFromActiveTarget = useCallback(async () => {
    const target = activeSession ? toTerminalTargetRequest(activeSession.target) : selectedTarget;

    if (!target) {
      setIsOpen(true);
      return;
    }

    const session = await tauriClient.terminalCreateSession({
      target: shellTarget(target),
      cols: DEFAULT_COLS,
      rows: DEFAULT_ROWS,
    });
    upsertSession(session);
  }, [activeSession, selectedTarget, upsertSession]);

  const value = useMemo<TerminalContextValue>(
    () => ({
      activeSession,
      activeSessionId,
      closePanel: () => setIsOpen(false),
      closeSession,
      findSessionForTarget,
      focusRequestId,
      hasOwnedServiceSession: (target) => Boolean(findSessionForTarget(target)),
      isOpen,
      launchService,
      newTabFromActiveTarget,
      openSelectedTarget,
      openPanel: () => setIsOpen(true),
      openTerminal,
      outputBySessionId,
      panelHeight,
      reloadShellEnvironment,
      resizeSession,
      restartSession,
      selectEnvironment,
      selectNextEnvironment: () => selectAdjacentEnvironment(1),
      selectNextSession,
      selectPreviousEnvironment: () => selectAdjacentEnvironment(-1),
      selectPreviousSession,
      sessionStates,
      sessions,
      setActiveSessionId,
      setPanelHeight,
      selectedTarget,
      setSelectedTarget,
      shellEnvironment,
      startedBy,
      stopSession,
      togglePanel: () => setIsOpen((current) => !current),
      writeToSession,
    }),
    [
      activeSession,
      activeSessionId,
      closeSession,
      findSessionForTarget,
      focusRequestId,
      isOpen,
      launchService,
      newTabFromActiveTarget,
      openSelectedTarget,
      openTerminal,
      outputBySessionId,
      panelHeight,
      reloadShellEnvironment,
      resizeSession,
      restartSession,
      selectAdjacentEnvironment,
      selectEnvironment,
      selectNextSession,
      selectPreviousSession,
      sessionStates,
      sessions,
      setActiveSessionId,
      setPanelHeight,
      setSelectedTarget,
      selectedTarget,
      shellEnvironment,
      startedBy,
      stopSession,
      writeToSession,
    ],
  );

  return <TerminalContext.Provider value={value}>{children}</TerminalContext.Provider>;
};

export function useTerminal() {
  const context = useContext(TerminalContext);
  if (!context) {
    throw new Error("useTerminal must be used within TerminalProvider");
  }
  return context;
}

export function toTerminalTargetRequest(target: TerminalSessionTargetDto): TerminalTargetRequest {
  return {
    kind: target.kind,
    projectId: target.projectId,
    environmentSlug: target.environmentSlug,
    serviceId: target.serviceId ?? null,
  };
}

export function terminalTargetMatches(
  sessionTarget: TerminalSessionTargetDto,
  target: TerminalTargetRequest,
) {
  return terminalTargetRequestMatches(toTerminalTargetRequest(sessionTarget), target);
}

export function terminalTargetRequestMatches(
  left: TerminalTargetRequest | null,
  right: TerminalTargetRequest | null,
) {
  if (!left || !right) {
    return false;
  }

  return (
    left.kind === right.kind &&
    left.projectId === right.projectId &&
    left.environmentSlug === right.environmentSlug &&
    (left.serviceId ?? null) === (right.serviceId ?? null)
  );
}

function targetEnvironmentMatches(left: TerminalTargetRequest, right: TerminalTargetRequest) {
  return left.projectId === right.projectId && left.environmentSlug === right.environmentSlug;
}

function loadStoredPanelHeight() {
  if (typeof window === "undefined") {
    return DEFAULT_PANEL_HEIGHT;
  }

  const parsed = Number(window.localStorage.getItem(TERMINAL_HEIGHT_STORAGE_KEY));
  return Number.isFinite(parsed) ? clampPanelHeight(parsed) : DEFAULT_PANEL_HEIGHT;
}

function clampPanelHeight(height: number) {
  const maxHeight =
    typeof window === "undefined"
      ? DEFAULT_PANEL_HEIGHT
      : Math.max(MIN_PANEL_HEIGHT, Math.floor(window.innerHeight * MAX_PANEL_HEIGHT_RATIO));
  return Math.min(maxHeight, Math.max(MIN_PANEL_HEIGHT, Math.round(height)));
}

function appendOutputChunk(
  current: OutputBySessionId,
  sessionId: string,
  chunk: Uint8Array,
): OutputBySessionId {
  const existing = current[sessionId] ?? [];
  const chunks = [...existing, chunk];
  let total = 0;
  const retained: Uint8Array[] = [];

  for (let index = chunks.length - 1; index >= 0; index -= 1) {
    const candidate = chunks[index];
    if (total + candidate.byteLength > MAX_BUFFER_BYTES && retained.length > 0) {
      break;
    }
    retained.unshift(candidate);
    total += candidate.byteLength;
  }

  return {
    ...current,
    [sessionId]: retained,
  };
}

// Tabs show one checkout at a time, so moving between tabs stays inside it.
function adjacentSessionId(
  sessions: TerminalSessionDto[],
  currentSessionId: string | null,
  offset: 1 | -1,
) {
  if (sessions.length === 0) return null;
  const current = sessions.find((session) => session.id === currentSessionId);
  if (!current) return sessions[0].id;
  const scoped = sessions.filter((session) =>
    sessionEnvironmentMatches(session.target, current.target),
  );
  return scoped[(scoped.indexOf(current) + offset + scoped.length) % scoped.length].id;
}

function sessionEnvironmentMatches(
  left: TerminalSessionTargetDto,
  right: TerminalSessionTargetDto,
) {
  return environmentKey(left) === environmentKey(right);
}

function environmentKey(target: { projectId: string; environmentSlug: string }) {
  return `${target.projectId}/${target.environmentSlug}`;
}

// A new tab is a plain shell at the checkout's root, even next to a service tab.
function shellTarget(target: TerminalTargetRequest): TerminalTargetRequest {
  return {
    kind: target.environmentSlug === "primary" ? "project" : "projectEnvironment",
    projectId: target.projectId,
    environmentSlug: target.environmentSlug,
    serviceId: null,
  };
}
