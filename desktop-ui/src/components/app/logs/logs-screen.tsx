"use client";

import { useQuery } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { useEffect, useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { bootstrapQueries, logsQueries } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { LogsDetail } from "./logs-detail";
import { useDividers } from "./logs-dividers";
import { LogsHeader, type LogsRequestDensity, type LogsViewMode } from "./logs-header";
import { LogsList } from "./logs-list";
import { parseRequests, type RequestEntry } from "./request-parser";

const TAIL_LINES = 1000;
const FOLLOW_POLL_MS = 2000;
const CLEAR_STORAGE_KEY = "roxy.logs.clearedAt";
const PROJECT_STORAGE_KEY = "roxy.logs.project";
const FOLLOW_STORAGE_KEY = "roxy.logs.follow";
const VIEW_STORAGE_KEY = "roxy.logs.view";
const DENSITY_STORAGE_KEY = "roxy.logs.density";

function loadStoredNumber(key: string): number | null {
  if (typeof window === "undefined") return null;
  const raw = window.localStorage.getItem(key);
  if (!raw) return null;
  const n = Number(raw);
  return Number.isFinite(n) ? n : null;
}

function loadStoredString(key: string): string | null {
  if (typeof window === "undefined") return null;
  return window.localStorage.getItem(key);
}

function persist(key: string, value: string | null) {
  try {
    if (value === null) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch {
    // ignore
  }
}

export function LogsScreen() {
  const navigate = useNavigate();
  const search = useSearch({ from: "/logs" });

  const [viewMode, setViewModeState] = useState<LogsViewMode>(
    () => search.view ?? (loadStoredString(VIEW_STORAGE_KEY) === "raw" ? "raw" : "requests"),
  );
  const [follow, setFollowState] = useState<boolean>(
    () => loadStoredString(FOLLOW_STORAGE_KEY) !== "0",
  );
  const [clearedAt, setClearedAtState] = useState<number | null>(() =>
    loadStoredNumber(CLEAR_STORAGE_KEY),
  );
  const [projectId, setProjectIdState] = useState<string>(
    () => search.projectId ?? loadStoredString(PROJECT_STORAGE_KEY) ?? "all",
  );
  const [selectedId, setSelectedIdState] = useState<number | null>(() =>
    typeof search.selectedId === "number" ? search.selectedId : null,
  );
  const [requestDensity, setRequestDensityState] = useState<LogsRequestDensity>(() =>
    loadStoredString(DENSITY_STORAGE_KEY) === "all" ? "all" : "focus",
  );

  const setFollow = (next: boolean) => {
    setFollowState(next);
    persist(FOLLOW_STORAGE_KEY, next ? "1" : "0");
  };
  const setClearedAt = (next: number | null) => {
    setClearedAtState(next);
    persist(CLEAR_STORAGE_KEY, next === null ? null : String(next));
  };
  const setProjectId = (next: string) => {
    setProjectIdState(next);
    persist(PROJECT_STORAGE_KEY, next);
    void navigate({
      to: "/logs",
      search: (previous) => ({
        ...previous,
        projectId: next === "all" ? undefined : next,
      }),
      replace: true,
    });
  };
  const setViewMode = (next: LogsViewMode) => {
    setViewModeState(next);
    persist(VIEW_STORAGE_KEY, next);
    void navigate({
      to: "/logs",
      search: (previous) => ({
        ...previous,
        view: next,
      }),
      replace: true,
    });
  };
  const setSelectedId = (next: number | null) => {
    setSelectedIdState(next);
    void navigate({
      to: "/logs",
      search: (previous) => ({
        ...previous,
        selectedId: next ?? undefined,
      }),
      replace: true,
    });
  };
  const setRequestDensity = (next: LogsRequestDensity) => {
    setRequestDensityState(next);
    persist(DENSITY_STORAGE_KEY, next);
  };

  const bootstrapQuery = useQuery(bootstrapQueries.read());
  const logsQuery = useQuery({
    ...logsQueries.read({ lines: TAIL_LINES, clear: false }),
    refetchInterval: follow ? FOLLOW_POLL_MS : false,
    refetchIntervalInBackground: false,
  });

  const domains = bootstrapQuery.data?.domains;
  const projects = bootstrapQuery.data?.projects;

  const requests = useMemo<RequestEntry[]>(
    () => parseRequests(logsQuery.data?.content ?? "", domains ?? []),
    [logsQuery.data?.content, domains],
  );
  const rawLines = useMemo(() => {
    const content = logsQuery.data?.content ?? "";
    if (!content) return [];
    return content.split("\n").filter((line) => line.trim().length > 0);
  }, [logsQuery.data?.content]);

  const scopedRequests = useMemo(() => {
    let out = requests;
    if (projectId !== "all") {
      out = out.filter((r) => r.projectId === projectId);
    }
    if (clearedAt !== null) {
      out = out.filter((r) => r.timestamp.getTime() > clearedAt);
    }
    return out;
  }, [requests, projectId, clearedAt]);

  const hiddenByClear = useMemo(() => {
    if (clearedAt === null) return 0;
    return requests.filter(
      (r) =>
        r.timestamp.getTime() <= clearedAt && (projectId === "all" || r.projectId === projectId),
    ).length;
  }, [requests, clearedAt, projectId]);
  const hiddenNoiseCount = useMemo(() => {
    if (requestDensity === "all") return 0;
    return scopedRequests.filter((r) => r.noise !== null).length;
  }, [requestDensity, scopedRequests]);
  const visible = useMemo(() => {
    if (requestDensity === "all") return scopedRequests;
    return scopedRequests.filter((r) => r.noise === null);
  }, [requestDensity, scopedRequests]);
  const issueCount = useMemo(
    () => visible.filter((request) => request.status >= 400).length,
    [visible],
  );

  const { dividers, add: addDivider, remove: removeDivider, clear: clearDividers } = useDividers();
  const visibleDividers = useMemo(() => {
    if (clearedAt === null) return dividers;
    return dividers.filter((d) => d.at > clearedAt);
  }, [dividers, clearedAt]);

  const selectedEntry = useMemo(
    () => (selectedId === null ? null : (visible.find((r) => r.id === selectedId) ?? null)),
    [visible, selectedId],
  );

  useEffect(() => {
    const nextProjectId = search.projectId ?? loadStoredString(PROJECT_STORAGE_KEY) ?? "all";
    setProjectIdState((current) => (current === nextProjectId ? current : nextProjectId));
  }, [search.projectId]);

  useEffect(() => {
    const nextViewMode =
      search.view ?? (loadStoredString(VIEW_STORAGE_KEY) === "raw" ? "raw" : "requests");
    setViewModeState((current) => (current === nextViewMode ? current : nextViewMode));
  }, [search.view]);

  useEffect(() => {
    const next = typeof search.selectedId === "number" ? search.selectedId : null;
    setSelectedIdState((current) => (current === next ? current : next));
  }, [search.selectedId]);

  // If the selected request has disappeared from the visible list (cleared,
  // filtered, etc.), clear the selection so the detail pane doesn't point to
  // a stale entry.
  useEffect(() => {
    if (selectedId === null) return;
    if (!visible.some((r) => r.id === selectedId)) {
      setSelectedIdState(null);
    }
  }, [visible, selectedId]);

  const handleClear = () => {
    setClearedAt(Date.now());
    setSelectedId(null);
  };
  const handleShowHidden = () => setClearedAt(null);
  const handleRefresh = () => {
    void logsQuery.refetch();
  };
  const handleMark = () => addDivider(Date.now());
  const handleToggleFollow = () => setFollow(!follow);

  const reduceMotion = useReducedMotion();
  const detailOpen = selectedEntry !== null;
  const detailEnterTransition = reduceMotion
    ? { duration: 0.12 }
    : { duration: 0.3, ease: [0.22, 1, 0.36, 1] as const };

  return (
    <div id="logs-screen" className="mx-auto flex h-full min-h-0 w-full max-w-4xl flex-col">
      <LogsHeader
        projects={projects ?? []}
        projectId={projectId}
        onProjectChange={setProjectId}
        viewMode={viewMode}
        onViewModeChange={setViewMode}
        follow={follow}
        onToggleFollow={handleToggleFollow}
        onMark={handleMark}
        onRefresh={handleRefresh}
        isRefreshing={logsQuery.isFetching && !follow}
        onClear={handleClear}
        onShowHidden={handleShowHidden}
        canShowHidden={clearedAt !== null}
        onClearDividers={clearDividers}
        canClearDividers={dividers.length > 0}
        requestDensity={requestDensity}
        onRequestDensityChange={setRequestDensity}
        visibleRequestCount={visible.length}
        hiddenNoiseCount={hiddenNoiseCount}
        issueCount={issueCount}
      />

      <div className="-mb-6 flex min-h-0 flex-1 overflow-hidden rounded-t-lg border border-border bg-background">
        {viewMode === "raw" ? (
          <RawView
            lines={rawLines}
            loading={logsQuery.isLoading}
            logFileExists={logsQuery.data?.exists ?? false}
            logPath={logsQuery.data?.path ?? null}
          />
        ) : (
          <>
            <div
              className={cn(
                "flex min-w-0 shrink-0 flex-col border-r transition-[width,border-color] duration-300 ease-[cubic-bezier(0.22,1,0.36,1)] motion-reduce:transition-none",
                detailOpen ? "w-[380px] border-border" : "w-full border-transparent",
              )}
            >
              {hiddenByClear > 0 ? (
                <button
                  type="button"
                  onClick={handleShowHidden}
                  className="flex w-full items-center justify-center gap-2 border-b border-border bg-muted/40 py-2 text-[11px] text-muted-foreground hover:bg-muted/70"
                >
                  {hiddenByClear} earlier request{hiddenByClear > 1 ? "s" : ""} hidden — show
                </button>
              ) : null}
              {hiddenNoiseCount > 0 ? (
                <button
                  type="button"
                  onClick={() => setRequestDensity("all")}
                  className="flex w-full items-center justify-center gap-2 border-b border-border bg-muted/20 py-2 text-[11px] text-muted-foreground hover:bg-muted/50"
                >
                  {hiddenNoiseCount} routine asset request{hiddenNoiseCount > 1 ? "s" : ""} hidden —
                  show all
                </button>
              ) : null}
              <div className="flex-1 overflow-auto">
                {visible.length === 0 && visibleDividers.length === 0 ? (
                  <RequestEmptyState
                    loading={logsQuery.isLoading}
                    projectSelected={projectId !== "all"}
                    hasClearCutoff={clearedAt !== null && hiddenByClear === 0}
                    logFileExists={logsQuery.data?.exists ?? false}
                    logPath={logsQuery.data?.path ?? null}
                    rawLogCount={rawLines.length}
                    hiddenNoiseCount={hiddenNoiseCount}
                    onShowRaw={() => setViewMode("raw")}
                    onShowAllRequests={() => setRequestDensity("all")}
                  />
                ) : (
                  <LogsList
                    entries={visible}
                    dividers={visibleDividers}
                    selectedId={selectedId}
                    onSelect={setSelectedId}
                    onRemoveDivider={removeDivider}
                    showProject={projectId === "all"}
                  />
                )}
              </div>
            </div>

            <AnimatePresence initial={false}>
              {detailOpen ? (
                <motion.div
                  key="logs-detail"
                  initial={reduceMotion ? { opacity: 0 } : { opacity: 0, x: 24 }}
                  animate={reduceMotion ? { opacity: 1 } : { opacity: 1, x: 0 }}
                  exit={reduceMotion ? { opacity: 0 } : { opacity: 0, x: 24 }}
                  transition={detailEnterTransition}
                  className="flex min-w-0 flex-1 flex-col overflow-hidden"
                >
                  <LogsDetail entry={selectedEntry} onClose={() => setSelectedId(null)} />
                </motion.div>
              ) : null}
            </AnimatePresence>
          </>
        )}
      </div>
    </div>
  );
}

function RawView({
  lines,
  loading,
  logFileExists,
  logPath,
}: {
  lines: string[];
  loading: boolean;
  logFileExists: boolean;
  logPath: string | null;
}) {
  if (lines.length === 0) {
    let title: string;
    let description: string;
    if (loading) {
      title = "Loading…";
      description = "";
    } else if (!logFileExists && logPath) {
      title = "No logs yet";
      description = `Nothing has been written to ${logPath} yet.`;
    } else {
      title = "No log lines yet";
      description = "New daemon output will appear here.";
    }

    return (
      <div className="flex h-full min-h-[200px] flex-1 flex-col items-center justify-center gap-2 px-6 text-center">
        <h3 className="text-sm font-semibold text-foreground">{title}</h3>
        {description ? (
          <p className="max-w-md text-xs text-muted-foreground">{description}</p>
        ) : null}
      </div>
    );
  }

  return (
    <div className="flex-1 overflow-auto">
      <ul className="divide-y divide-border">
        {lines.map((line, index) => (
          <li
            key={`${index}-${line}`}
            className="px-4 py-2 font-mono text-[11px] leading-5 whitespace-pre-wrap text-foreground"
          >
            {line}
          </li>
        ))}
      </ul>
    </div>
  );
}

function RequestEmptyState({
  loading,
  projectSelected,
  hasClearCutoff,
  logFileExists,
  logPath,
  rawLogCount,
  hiddenNoiseCount,
  onShowRaw,
  onShowAllRequests,
}: {
  loading: boolean;
  projectSelected: boolean;
  hasClearCutoff: boolean;
  logFileExists: boolean;
  logPath: string | null;
  rawLogCount: number;
  hiddenNoiseCount: number;
  onShowRaw: () => void;
  onShowAllRequests: () => void;
}) {
  let title: string;
  let description: string;
  if (loading) {
    title = "Loading…";
    description = "";
  } else if (hiddenNoiseCount > 0) {
    title = "Only routine asset requests are flowing right now";
    description = "Focus mode is hiding the noisy dev-server traffic so real requests stand out.";
  } else if (hasClearCutoff) {
    title = "No new requests since you cleared";
    description = "New requests to the proxy will appear here.";
  } else if (projectSelected) {
    title = "No requests for this project yet";
    description = "Requests hitting this project's domains will appear here.";
  } else if (rawLogCount > 0) {
    title = "No request entries yet";
    description =
      "The daemon is writing logs, but none of the current lines are proxy request events.";
  } else if (!logFileExists && logPath) {
    title = "No logs yet";
    description = `Nothing has been written to ${logPath} yet. Start the daemon and hit a Localify domain to generate request entries.`;
  } else {
    title = "No requests yet";
    description = "Requests through the proxy will be recorded here.";
  }
  return (
    <div className="flex h-full min-h-[200px] flex-col items-center justify-center gap-2 px-6 text-center">
      <h3 className="text-sm font-semibold text-foreground">{title}</h3>
      {description ? <p className="max-w-md text-xs text-muted-foreground">{description}</p> : null}
      {hiddenNoiseCount > 0 ? (
        <Button variant="outline" size="sm" onClick={onShowAllRequests}>
          Show all requests
        </Button>
      ) : null}
      {rawLogCount > 0 ? (
        <Button variant="outline" size="sm" onClick={onShowRaw}>
          Show raw logs
        </Button>
      ) : null}
    </div>
  );
}
