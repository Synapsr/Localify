"use client";

import { useQueryClient } from "@tanstack/react-query";
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

import type {
  TerminalExitPayload,
  TerminalOutputPayload,
  TerminalSessionDto,
  TerminalSessionTargetDto,
  TerminalTargetRequest,
} from "@/types/localify";

import { bootstrapQueries } from "@/lib/queries";
import { tauriClient } from "@/lib/tauri-client";

const DEFAULT_COLS = 120;
const DEFAULT_ROWS = 32;
const DEFAULT_PANEL_HEIGHT = 360;
const MIN_PANEL_HEIGHT = 220;
const MAX_PANEL_HEIGHT_RATIO = 0.82;
const MAX_BUFFER_BYTES = 1_000_000;
const TERMINAL_HEIGHT_STORAGE_KEY = "localify.terminal.height";

type OutputBySessionId = Record<string, Uint8Array[]>;

interface TerminalContextValue {
  activeSession: TerminalSessionDto | null;
  activeSessionId: string | null;
  closePanel: () => void;
  closeSession: (sessionId?: string) => Promise<void>;
  findSessionForTarget: (target: TerminalTargetRequest) => TerminalSessionDto | null;
  hasOwnedServiceSession: (target: TerminalTargetRequest) => boolean;
  interruptSession: (sessionId?: string) => Promise<void>;
  isOpen: boolean;
  launchService: (target: TerminalTargetRequest) => Promise<TerminalSessionDto>;
  newTabFromActiveTarget: () => Promise<void>;
  openSelectedTarget: () => Promise<TerminalSessionDto | null>;
  openPanel: () => void;
  openTerminal: (target: TerminalTargetRequest) => Promise<TerminalSessionDto>;
  outputBySessionId: OutputBySessionId;
  panelHeight: number;
  resizeSession: (sessionId: string, cols: number, rows: number) => Promise<void>;
  selectNextSession: () => void;
  selectPreviousSession: () => void;
  sessions: TerminalSessionDto[];
  setActiveSessionId: (sessionId: string) => void;
  setPanelHeight: (height: number) => void;
  selectedTarget: TerminalTargetRequest | null;
  setSelectedTarget: (target: TerminalTargetRequest | null) => void;
  sidebarCollapsed: boolean;
  togglePanel: () => void;
  toggleSidebar: () => void;
  writeToSession: (sessionId: string, data: string) => Promise<void>;
}

const TerminalContext = createContext<TerminalContextValue | null>(null);

export const TerminalProvider = ({ children }: PropsWithChildren) => {
  const queryClient = useQueryClient();
  const [sessions, setSessions] = useState<TerminalSessionDto[]>([]);
  const [outputBySessionId, setOutputBySessionId] = useState<OutputBySessionId>({});
  const [activeSessionId, setActiveSessionIdState] = useState<string | null>(null);
  const [isOpen, setIsOpen] = useState(false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [panelHeight, setPanelHeightState] = useState(loadStoredPanelHeight);
  const [selectedTarget, setSelectedTargetState] = useState<TerminalTargetRequest | null>(null);
  const sessionsRef = useRef(sessions);

  useEffect(() => {
    sessionsRef.current = sessions;
  }, [sessions]);

  useEffect(() => {
    let cancelled = false;

    void tauriClient
      .terminalListSessions()
      .then((nextSessions) => {
        if (cancelled) return;
        const initialSession = nextSessions[0] ?? null;
        setSessions(nextSessions);
        setActiveSessionIdState((current) => current ?? initialSession?.id ?? null);
        setSelectedTargetState(
          (current) =>
            current ?? (initialSession ? toTerminalTargetRequest(initialSession.target) : null),
        );
      })
      .catch(() => {
        if (!cancelled) {
          setSessions([]);
        }
      });

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let unlistenOutput: (() => void) | null = null;
    let unlistenExit: (() => void) | null = null;
    let cancelled = false;

    void listen<TerminalOutputPayload>("terminal-output", (event) => {
      if (cancelled) return;
      const chunk = new Uint8Array(event.payload.bytes);
      setOutputBySessionId((current) => appendOutputChunk(current, event.payload.sessionId, chunk));
    }).then((unlisten) => {
      unlistenOutput = unlisten;
    });

    void listen<TerminalExitPayload>("terminal-exit", (event) => {
      if (cancelled) return;
      setSessions((current) =>
        current.map((session) =>
          session.id === event.payload.sessionId ? { ...session, status: "exited" } : session,
        ),
      );
    }).then((unlisten) => {
      unlistenExit = unlisten;
    });

    return () => {
      cancelled = true;
      unlistenOutput?.();
      unlistenExit?.();
    };
  }, []);

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

  const upsertSession = useCallback((session: TerminalSessionDto) => {
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
  }, []);

  const findSessionForTarget = useCallback((target: TerminalTargetRequest) => {
    return (
      sessionsRef.current.find(
        (session) => session.status === "running" && terminalTargetMatches(session.target, target),
      ) ?? null
    );
  }, []);

  const openTerminal = useCallback(
    async (target: TerminalTargetRequest) => {
      const existing = findSessionForTarget(target);
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
    [findSessionForTarget, setActiveSessionId, upsertSession],
  );

  const launchService = useCallback(
    async (target: TerminalTargetRequest) => {
      const result = await tauriClient.terminalLaunchService({
        target,
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
      });
      upsertSession(result.session);
      await queryClient.invalidateQueries({ queryKey: bootstrapQueries.all() });
      return result.session;
    },
    [queryClient, upsertSession],
  );

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

  const interruptSession = useCallback(
    async (sessionId = activeSessionId ?? undefined) => {
      if (!sessionId) return;
      await tauriClient.terminalInterrupt({ sessionId });
      await queryClient.invalidateQueries({ queryKey: bootstrapQueries.all() });
    },
    [activeSessionId, queryClient],
  );

  const closeSession = useCallback(
    async (sessionId = activeSessionId ?? undefined) => {
      if (!sessionId) return;
      const currentSessions = sessionsRef.current;
      const session = currentSessions.find((candidate) => candidate.id === sessionId);
      if (!session) return;

      if (
        session.status === "running" &&
        session.target.kind === "service" &&
        !window.confirm("Close this service terminal and stop its process?")
      ) {
        return;
      }

      await tauriClient.terminalClose({ sessionId });
      const next = currentSessions.filter((candidate) => candidate.id !== sessionId);
      setSessions(next);
      setActiveSessionIdState((currentActive) => {
        if (currentActive !== sessionId) return currentActive;
        const nextActive =
          lastSessionMatchingEnvironment(next, session.target) ?? lastSession(next) ?? null;
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

  const newTabFromActiveTarget = useCallback(async () => {
    const active = sessionsRef.current.find((session) => session.id === activeSessionId);
    const target =
      selectedTarget &&
      (!active || !targetEnvironmentMatches(toTerminalTargetRequest(active.target), selectedTarget))
        ? selectedTarget
        : active
          ? toTerminalTargetRequest(active.target)
          : selectedTarget;

    if (!target) {
      setIsOpen(true);
      return;
    }

    const session = await tauriClient.terminalCreateSession({
      target,
      cols: DEFAULT_COLS,
      rows: DEFAULT_ROWS,
    });
    upsertSession(session);
  }, [activeSessionId, selectedTarget, upsertSession]);

  const activeSession = useMemo(
    () => sessions.find((session) => session.id === activeSessionId) ?? null,
    [activeSessionId, sessions],
  );

  const value = useMemo<TerminalContextValue>(
    () => ({
      activeSession,
      activeSessionId,
      closePanel: () => setIsOpen(false),
      closeSession,
      findSessionForTarget,
      hasOwnedServiceSession: (target) => Boolean(findSessionForTarget(target)),
      interruptSession,
      isOpen,
      launchService,
      newTabFromActiveTarget,
      openSelectedTarget,
      openPanel: () => setIsOpen(true),
      openTerminal,
      outputBySessionId,
      panelHeight,
      resizeSession,
      selectNextSession,
      selectPreviousSession,
      sessions,
      setActiveSessionId,
      setPanelHeight,
      selectedTarget,
      setSelectedTarget,
      sidebarCollapsed,
      togglePanel: () => setIsOpen((current) => !current),
      toggleSidebar: () => setSidebarCollapsed((current) => !current),
      writeToSession,
    }),
    [
      activeSession,
      activeSessionId,
      closeSession,
      findSessionForTarget,
      interruptSession,
      isOpen,
      launchService,
      newTabFromActiveTarget,
      openSelectedTarget,
      openTerminal,
      outputBySessionId,
      panelHeight,
      resizeSession,
      selectNextSession,
      selectPreviousSession,
      sessions,
      setActiveSessionId,
      setPanelHeight,
      setSelectedTarget,
      selectedTarget,
      sidebarCollapsed,
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

function adjacentSessionId(
  sessions: TerminalSessionDto[],
  currentSessionId: string | null,
  offset: 1 | -1,
) {
  if (sessions.length === 0) return null;
  const currentIndex = sessions.findIndex((session) => session.id === currentSessionId);
  if (currentIndex === -1) return sessions[0].id;
  const currentSession = sessions[currentIndex];
  const scopedSessions = sessions.filter((session) =>
    sessionEnvironmentMatches(session.target, currentSession.target),
  );
  const scopedIndex = scopedSessions.findIndex((session) => session.id === currentSessionId);
  if (scopedIndex === -1) return scopedSessions[0]?.id ?? sessions[0].id;
  return scopedSessions[(scopedIndex + offset + scopedSessions.length) % scopedSessions.length].id;
}

function sessionEnvironmentMatches(
  left: TerminalSessionTargetDto,
  right: TerminalSessionTargetDto,
) {
  return left.projectId === right.projectId && left.environmentSlug === right.environmentSlug;
}

function lastSession(sessions: TerminalSessionDto[]) {
  return sessions.length > 0 ? sessions[sessions.length - 1] : null;
}

function lastSessionMatchingEnvironment(
  sessions: TerminalSessionDto[],
  target: TerminalSessionTargetDto,
) {
  for (let index = sessions.length - 1; index >= 0; index -= 1) {
    const session = sessions[index];
    if (sessionEnvironmentMatches(session.target, target)) {
      return session;
    }
  }
  return null;
}
