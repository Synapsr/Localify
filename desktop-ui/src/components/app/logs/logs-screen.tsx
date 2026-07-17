"use client";

import { useQuery } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

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
const CLEAR_STORAGE_KEY = "localify.logs.clearedAt";
const PROJECT_STORAGE_KEY = "localify.logs.project";
const DOMAIN_STORAGE_KEY = "localify.logs.domain";
const FOLLOW_STORAGE_KEY = "localify.logs.follow";
const VIEW_STORAGE_KEY = "localify.logs.view";
const DENSITY_STORAGE_KEY = "localify.logs.density";
const PATH_PREFIXES_STORAGE_KEY = "localify.logs.pathPrefixes";

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

function parsePathPrefixes(value: string): string[] {
  return value
    .split(/[\s,]+/)
    .map((prefix) => prefix.trim())
    .filter((prefix) => prefix.length > 0)
    .map((prefix) => (prefix.startsWith("/") ? prefix : `/${prefix}`));
}

function matchesPathPrefix(entry: RequestEntry, prefixes: string[]): boolean {
  if (!entry.path || prefixes.length === 0) return false;
  return prefixes.some((prefix) => entry.path?.startsWith(prefix) ?? false);
}

export function LogsScreen() {
  const navigate = useNavigate();
  const search = useSearch({ from: "/logs" });
  const requestListScrollRef = useRef<HTMLDivElement | null>(null);

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
  const [domain, setDomainState] = useState<string>(
    () => search.domain ?? loadStoredString(DOMAIN_STORAGE_KEY) ?? "all",
  );
  const [selectedId, setSelectedIdState] = useState<number | null>(() =>
    typeof search.selectedId === "number" ? search.selectedId : null,
  );
  const [requestDensity, setRequestDensityState] = useState<LogsRequestDensity>(() =>
    loadStoredString(DENSITY_STORAGE_KEY) === "all" ? "all" : "focus",
  );
  const [pathPrefixes, setPathPrefixesState] = useState<string>(
    () => loadStoredString(PATH_PREFIXES_STORAGE_KEY) ?? "",
  );
  const [latestDividerElement, setLatestDividerElement] = useState<HTMLLIElement | null>(null);
  const [latestDividerVisible, setLatestDividerVisible] = useState(true);

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
    setDomainState("all");
    persist(PROJECT_STORAGE_KEY, next);
    persist(DOMAIN_STORAGE_KEY, "all");
    void navigate({
      to: "/logs",
      search: (previous) => ({
        ...previous,
        projectId: next === "all" ? undefined : next,
        domain: undefined,
        selectedId: undefined,
      }),
      replace: true,
    });
  };
  const setDomain = useCallback((next: string) => {
    setDomainState(next);
    persist(DOMAIN_STORAGE_KEY, next);
    void navigate({
      to: "/logs",
      search: (previous) => ({
        ...previous,
        domain: next === "all" ? undefined : next,
        selectedId: undefined,
      }),
      replace: true,
    });
  }, [navigate]);
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
  const setPathPrefixes = (next: string) => {
    setPathPrefixesState(next);
    persist(PATH_PREFIXES_STORAGE_KEY, next.trim().length === 0 ? null : next);
  };

  const bootstrapQuery = useQuery(bootstrapQueries.read());
  const logsQuery = useQuery({
    ...logsQueries.read({ lines: TAIL_LINES, clear: false }),
    refetchInterval: follow ? FOLLOW_POLL_MS : false,
    refetchIntervalInBackground: false,
  });

  const domains = bootstrapQuery.data?.domains;
  const projects = bootstrapQuery.data?.projects;
  const projectDomains = useMemo(() => {
    if (!domains || projectId === "all") return [];
    return domains
      .filter((d) => d.projectId === projectId)
      .sort((left, right) => left.registration.domain.localeCompare(right.registration.domain));
  }, [domains, projectId]);
  const pathPrefixFilters = useMemo(() => parsePathPrefixes(pathPrefixes), [pathPrefixes]);

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
    if (projectId !== "all" && domain !== "all") {
      out = out.filter((r) => r.host === domain);
    }
    if (clearedAt !== null) {
      out = out.filter((r) => r.timestamp.getTime() > clearedAt);
    }
    return out;
  }, [requests, projectId, domain, clearedAt]);

  const hiddenByClear = useMemo(() => {
    if (clearedAt === null) return 0;
    return requests.filter(
      (r) =>
        r.timestamp.getTime() <= clearedAt &&
        (projectId === "all" || r.projectId === projectId) &&
        (projectId === "all" || domain === "all" || r.host === domain),
    ).length;
  }, [requests, clearedAt, projectId, domain]);
  const hiddenNoiseCount = useMemo(() => {
    if (requestDensity === "all") return 0;
    return scopedRequests.filter(
      (r) => r.noise !== null && !matchesPathPrefix(r, pathPrefixFilters),
    ).length;
  }, [requestDensity, scopedRequests, pathPrefixFilters]);
  const hiddenPrefixCount = useMemo(
    () => scopedRequests.filter((r) => matchesPathPrefix(r, pathPrefixFilters)).length,
    [scopedRequests, pathPrefixFilters],
  );
  const visible = useMemo(() => {
    let out = scopedRequests;
    if (pathPrefixFilters.length > 0) {
      out = out.filter((r) => !matchesPathPrefix(r, pathPrefixFilters));
    }
    if (requestDensity === "all") return out;
    return out.filter((r) => r.noise === null);
  }, [requestDensity, scopedRequests, pathPrefixFilters]);
  const issueCount = useMemo(
    () => visible.filter((request) => request.status >= 400).length,
    [visible],
  );

  const { dividers, add: addDivider, remove: removeDivider, clear: clearDividers } = useDividers();
  const visibleDividers = useMemo(() => {
    if (clearedAt === null) return dividers;
    return dividers.filter((d) => d.at > clearedAt);
  }, [dividers, clearedAt]);
  const latestVisibleDivider = useMemo(() => {
    if (visibleDividers.length === 0) return null;
    return visibleDividers.reduce((latest, divider) => (divider.at > latest.at ? divider : latest));
  }, [visibleDividers]);
  const setLatestDividerRef = useCallback((node: HTMLLIElement | null) => {
    setLatestDividerElement(node);
  }, []);

  const selectedEntry = useMemo(
    () => (selectedId === null ? null : (visible.find((r) => r.id === selectedId) ?? null)),
    [visible, selectedId],
  );

  useEffect(() => {
    const nextProjectId = search.projectId ?? loadStoredString(PROJECT_STORAGE_KEY) ?? "all";
    setProjectIdState((current) => (current === nextProjectId ? current : nextProjectId));
  }, [search.projectId]);

  useEffect(() => {
    const nextDomain = search.domain ?? loadStoredString(DOMAIN_STORAGE_KEY) ?? "all";
    setDomainState((current) => (current === nextDomain ? current : nextDomain));
  }, [search.domain]);

  useEffect(() => {
    if (domain === "all") return;
    if (projectId === "all" || projectDomains.length === 0) {
      setDomain("all");
      return;
    }
    if (!projectDomains.some((d) => d.registration.domain === domain)) {
      setDomain("all");
    }
  }, [domain, projectId, projectDomains, setDomain]);

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

  useEffect(() => {
    const root = requestListScrollRef.current;
    const target = latestDividerElement;
    if (!root || !target) {
      setLatestDividerVisible(true);
      return;
    }

    const observer = new IntersectionObserver(
      ([entry]) => {
        setLatestDividerVisible(entry?.isIntersecting ?? true);
      },
      {
        root,
        threshold: 0.95,
      },
    );
    observer.observe(target);
    return () => observer.disconnect();
  }, [latestDividerElement]);

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
  const canJumpToLatestMarker =
    viewMode === "requests" && latestVisibleDivider !== null && !latestDividerVisible;
  const handleJumpToLatestMarker = () => {
    latestDividerElement?.scrollIntoView({
      behavior: reduceMotion ? "auto" : "smooth",
      block: "center",
    });
  };
  const detailEnterTransition = reduceMotion
    ? { duration: 0.12 }
    : { duration: 0.3, ease: [0.22, 1, 0.36, 1] as const };

  return (
    <div id="logs-screen" className="mx-auto flex h-full min-h-0 w-full max-w-4xl flex-col">
      <LogsHeader
        projects={projects ?? []}
        projectId={projectId}
        onProjectChange={setProjectId}
        domains={projectDomains}
        domain={domain}
        onDomainChange={setDomain}
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
        pathPrefixes={pathPrefixes}
        onPathPrefixesChange={setPathPrefixes}
        visibleRequestCount={visible.length}
        hiddenNoiseCount={hiddenNoiseCount}
        hiddenPrefixCount={hiddenPrefixCount}
        issueCount={issueCount}
      />

      <div className="-mb-6 flex min-h-0 flex-1 overflow-hidden rounded-t-lg border border-border bg-background">
        {viewMode === "raw" ? (
          <RawView
            lines={[...rawLines].reverse()}
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
              {hiddenPrefixCount > 0 ? (
                <button
                  type="button"
                  onClick={() => setPathPrefixes("")}
                  className="flex w-full items-center justify-center gap-2 border-b border-border bg-muted/20 py-2 text-[11px] text-muted-foreground hover:bg-muted/50"
                >
                  {hiddenPrefixCount} request{hiddenPrefixCount > 1 ? "s" : ""} hidden by prefix —
                  show
                </button>
              ) : null}
              <div className="relative min-h-0 flex-1">
                <div ref={requestListScrollRef} className="h-full overflow-auto">
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
                      latestDividerId={latestVisibleDivider?.id ?? null}
                      latestDividerRef={setLatestDividerRef}
                    />
                  )}
                </div>
                <AnimatePresence initial={false}>
                  {canJumpToLatestMarker ? (
                    <motion.div
                      key="latest-marker-indicator"
                      initial={reduceMotion ? { opacity: 0 } : { opacity: 0, y: 8 }}
                      animate={reduceMotion ? { opacity: 1 } : { opacity: 1, y: 0 }}
                      exit={reduceMotion ? { opacity: 0 } : { opacity: 0, y: 8 }}
                      transition={
                        reduceMotion
                          ? { duration: 0.12 }
                          : { duration: 0.2, ease: [0.22, 1, 0.36, 1] as const }
                      }
                      className="pointer-events-none absolute right-3 bottom-18 left-3 flex justify-center"
                    >
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        onClick={handleJumpToLatestMarker}
                        className="pointer-events-auto rounded-full bg-background/95 text-muted-foreground shadow-lg shadow-black/10 backdrop-blur transition-colors"
                      >
                        Latest marker
                      </Button>
                    </motion.div>
                  ) : null}
                </AnimatePresence>
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
