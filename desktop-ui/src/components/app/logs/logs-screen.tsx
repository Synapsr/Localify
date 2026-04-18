import { useQuery } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { useEffect, useMemo, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { bootstrapQueries, logsQueries } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { formatClock, parseRequests, type RequestEntry } from "./request-parser";

const TAIL_LINES = 1000;
const FOLLOW_POLL_MS = 2000;
const CLEAR_STORAGE_KEY = "roxy.logs.clearedAt";
const PROJECT_STORAGE_KEY = "roxy.logs.project";
const FOLLOW_STORAGE_KEY = "roxy.logs.follow";
const VIEW_STORAGE_KEY = "roxy.logs.view";

type LogsViewMode = "requests" | "raw";

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
  const [viewMode, setViewModeState] = useState<LogsViewMode>(() =>
    search.view ?? (loadStoredString(VIEW_STORAGE_KEY) === "raw" ? "raw" : "requests"),
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

  const visible = useMemo(() => {
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
  const renderedLineCount = viewMode === "raw" ? rawLines.length : visible.length;

  const scrollRef = useRef<HTMLDivElement | null>(null);

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
    if (!follow) return;
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [renderedLineCount, follow]);

  const handleClear = () => setClearedAt(Date.now());
  const handleShowHidden = () => setClearedAt(null);
  const handleRefresh = () => {
    void logsQuery.refetch();
  };

  return (
    <div className="-mx-6 -mb-6 flex h-full min-h-0 flex-col bg-background">
      <header className="flex items-center justify-between gap-3 border-b border-border px-4 pt-4 pb-3">
        <div className="flex items-center gap-3">
          <div>
            <h1 className="text-2xl font-semibold tracking-tight">Logs</h1>
            <p className="text-xs text-muted-foreground">
              {viewMode === "requests" ? "Structured proxy requests" : "Raw daemon output"}
            </p>
          </div>
          <span
            className={cn(
              "inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-[11px] font-medium",
              follow
                ? "border-emerald-200 bg-emerald-50 text-emerald-700"
                : "border-neutral-200 bg-neutral-50 text-neutral-600",
            )}
          >
            <span
              className={cn(
                "size-1.5 rounded-full",
                follow ? "animate-pulse bg-emerald-500" : "bg-neutral-400",
              )}
            />
            {follow ? "Live" : "Paused"}
          </span>
        </div>

        <div className="flex items-center gap-2">
          <div className="flex items-center rounded-md border border-border p-0.5">
            <Button
              variant={viewMode === "requests" ? "primary" : "ghost"}
              size="sm"
              onClick={() => setViewMode("requests")}
            >
              Requests
            </Button>
            <Button
              variant={viewMode === "raw" ? "primary" : "ghost"}
              size="sm"
              onClick={() => setViewMode("raw")}
            >
              Raw
            </Button>
          </div>
          {viewMode === "requests" ? (
            <select
              value={projectId}
              onChange={(e) => setProjectId(e.target.value)}
              aria-label="Filter by project"
              className="h-8 rounded-md border border-border bg-background px-2 text-xs"
            >
              <option value="all">All projects</option>
              {(projects ?? []).map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          ) : null}
          <Button
            variant={follow ? "primary" : "outline"}
            size="sm"
            onClick={() => setFollow(!follow)}
          >
            {follow ? "Following" : "Follow"}
          </Button>
          <Button
            variant="outline"
            size="sm"
            loading={logsQuery.isFetching && !follow}
            onClick={handleRefresh}
          >
            Refresh
          </Button>
          {viewMode === "requests" ? (
            <Button variant="ghost" size="sm" onClick={handleClear}>
              Clear
            </Button>
          ) : null}
        </div>
      </header>

      <div ref={scrollRef} className="flex-1 overflow-auto">
        {viewMode === "requests" && hiddenByClear > 0 ? (
          <button
            type="button"
            onClick={handleShowHidden}
            className="flex w-full items-center justify-center gap-2 border-b border-border bg-muted/40 py-2 text-[11px] text-muted-foreground hover:bg-muted/70"
          >
            {hiddenByClear} earlier request{hiddenByClear > 1 ? "s" : ""} hidden — show
          </button>
        ) : null}

        {viewMode === "raw" ? (
          rawLines.length === 0 ? (
            <RawEmptyState
              loading={logsQuery.isLoading}
              logFileExists={logsQuery.data?.exists ?? false}
              logPath={logsQuery.data?.path ?? null}
            />
          ) : (
            <RawLogList lines={rawLines} />
          )
        ) : visible.length === 0 ? (
          <RequestEmptyState
            loading={logsQuery.isLoading}
            projectSelected={projectId !== "all"}
            hasClearCutoff={clearedAt !== null && hiddenByClear === 0}
            logFileExists={logsQuery.data?.exists ?? false}
            logPath={logsQuery.data?.path ?? null}
            rawLogCount={rawLines.length}
            onShowRaw={() => setViewMode("raw")}
          />
        ) : (
          <ul className="divide-y divide-border">
            {visible.map((r) => (
              <RequestRow key={r.id} entry={r} showProject={projectId === "all"} />
            ))}
          </ul>
        )}
      </div>
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
  onShowRaw,
}: {
  loading: boolean;
  projectSelected: boolean;
  hasClearCutoff: boolean;
  logFileExists: boolean;
  logPath: string | null;
  rawLogCount: number;
  onShowRaw: () => void;
}) {
  let title: string;
  let description: string;
  if (loading) {
    title = "Loading…";
    description = "";
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
      {rawLogCount > 0 ? (
        <Button variant="outline" size="sm" onClick={onShowRaw}>
          Show raw logs
        </Button>
      ) : null}
    </div>
  );
}

function RawEmptyState({
  loading,
  logFileExists,
  logPath,
}: {
  loading: boolean;
  logFileExists: boolean;
  logPath: string | null;
}) {
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
    <div className="flex h-full min-h-[200px] flex-col items-center justify-center gap-2 px-6 text-center">
      <h3 className="text-sm font-semibold text-foreground">{title}</h3>
      {description ? <p className="max-w-md text-xs text-muted-foreground">{description}</p> : null}
    </div>
  );
}

function RawLogList({ lines }: { lines: string[] }) {
  return (
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
  );
}

function RequestRow({ entry, showProject }: { entry: RequestEntry; showProject: boolean }) {
  const statusTone = statusColor(entry.status);
  return (
    <li className="flex items-center gap-3 px-4 py-2 text-xs hover:bg-muted/40">
      <span className="w-[64px] shrink-0 font-mono text-[11px] text-muted-foreground tabular-nums select-none">
        {formatClock(entry.timestamp)}
      </span>
      <span
        className={cn(
          "inline-flex h-[20px] min-w-[44px] items-center justify-center rounded border px-1.5 font-mono text-[10px] font-semibold tabular-nums",
          statusTone.badge,
        )}
        title={statusTitle(entry.status, entry.kind)}
      >
        {entry.status}
      </span>
      {entry.method ? (
        <span className="inline-flex h-[20px] min-w-[44px] items-center justify-center rounded bg-muted px-1.5 font-mono text-[10px] font-semibold text-foreground/80">
          {entry.method}
        </span>
      ) : null}
      <div className="min-w-0 flex-1 truncate font-mono text-[12px]">
        <span className="text-foreground">{entry.host}</span>
        {entry.path ? <span className="text-muted-foreground">{entry.path}</span> : null}
        {entry.kind !== "completed" ? (
          <span className="ml-2 text-[11px] text-amber-700">
            {entry.kind === "no-route" ? "no route" : "not registered"}
          </span>
        ) : null}
      </div>
      {showProject && entry.projectName ? (
        <span className="shrink-0 rounded-full bg-muted px-2 py-0.5 text-[10px] font-medium text-muted-foreground">
          {entry.projectName}
        </span>
      ) : null}
    </li>
  );
}

function statusColor(status: number): { badge: string } {
  if (status >= 500) return { badge: "bg-red-50 text-red-700 border-red-200" };
  if (status >= 400) return { badge: "bg-amber-50 text-amber-700 border-amber-200" };
  if (status >= 300) return { badge: "bg-sky-50 text-sky-700 border-sky-200" };
  if (status >= 200) return { badge: "bg-emerald-50 text-emerald-700 border-emerald-200" };
  return { badge: "bg-neutral-50 text-neutral-700 border-neutral-200" };
}

function statusTitle(status: number, kind: RequestEntry["kind"]): string {
  if (kind === "domain-not-registered") return "Domain not registered";
  if (kind === "no-route") return "No route found";
  return `HTTP ${status}`;
}
