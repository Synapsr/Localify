"use client";

import { useMemo, type RefCallback } from "react";

import { XIcon } from "@/components/icons";
import { cn } from "@/lib/utils";

import type { Divider } from "./logs-dividers";

import { formatClock, type RequestEntry } from "./request-parser";

interface LogsListProps {
  entries: RequestEntry[];
  dividers: Divider[];
  selectedId: number | null;
  onSelect: (id: number) => void;
  onRemoveDivider: (id: string) => void;
  showProject: boolean;
  latestDividerId: string | null;
  latestDividerRef: RefCallback<HTMLLIElement>;
}

type Row =
  | { type: "request"; at: number; entry: RequestEntry }
  | { type: "divider"; at: number; divider: Divider };

export const LogsList = ({
  entries,
  dividers,
  selectedId,
  onSelect,
  onRemoveDivider,
  showProject,
  latestDividerId,
  latestDividerRef,
}: LogsListProps) => {
  const rows = useMemo<Row[]>(() => {
    const merged: Row[] = [
      ...entries.map<Row>((entry) => ({
        type: "request",
        at: entry.timestamp.getTime(),
        entry,
      })),
      ...dividers.map<Row>((divider) => ({
        type: "divider",
        at: divider.at,
        divider,
      })),
    ];
    merged.sort((a, b) => b.at - a.at);
    return merged;
  }, [entries, dividers]);

  return (
    <ul className="divide-y divide-border">
      {rows.map((row) =>
        row.type === "request" ? (
          <RequestRow
            key={`req-${row.entry.id}`}
            entry={row.entry}
            selected={row.entry.id === selectedId}
            onSelect={onSelect}
            showProject={showProject}
          />
        ) : (
          <DividerRow
            key={`div-${row.divider.id}`}
            divider={row.divider}
            onRemove={onRemoveDivider}
            refCallback={row.divider.id === latestDividerId ? latestDividerRef : undefined}
          />
        ),
      )}
    </ul>
  );
};

function RequestRow({
  entry,
  selected,
  onSelect,
  showProject,
}: {
  entry: RequestEntry;
  selected: boolean;
  onSelect: (id: number) => void;
  showProject: boolean;
}) {
  const tone = statusColor(entry.status);
  return (
    <li
      className={cn(
        "group relative border-l-2 border-l-transparent",
        selected
          ? "border-l-primary bg-muted/60"
          : entry.noise
            ? "hover:bg-muted/25"
            : "hover:bg-muted/40",
      )}
    >
      <button
        type="button"
        onClick={() => onSelect(entry.id)}
        className="flex w-full items-center gap-2 px-3 py-2 text-left text-xs"
      >
        <span className="w-[52px] shrink-0 font-mono text-[10px] text-muted-foreground tabular-nums">
          {formatClock(entry.timestamp)}
        </span>
        <span
          className={cn(
            "inline-flex h-[20px] min-w-[38px] items-center justify-center rounded border px-1 font-mono text-[10px] font-semibold tabular-nums",
            tone.badge,
          )}
        >
          {entry.status}
        </span>
        {entry.method ? (
          <span className="inline-flex h-[20px] min-w-[40px] items-center justify-center rounded bg-muted px-1 font-mono text-[10px] font-semibold text-foreground/80">
            {entry.method}
          </span>
        ) : null}
        <div className="min-w-0 flex-1 truncate font-mono text-[12px]">
          {entry.path ? (
            <span className={cn(entry.noise ? "text-muted-foreground" : "text-foreground")}>
              {entry.path}
            </span>
          ) : (
            <span className="text-muted-foreground">{entry.host}</span>
          )}
        </div>
        {entry.noise ? (
          <span className="hidden shrink-0 rounded-full border border-border/70 bg-muted/30 px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground xl:inline-flex">
            {entry.noise.label}
          </span>
        ) : null}
        {showProject && entry.projectName ? (
          <span className="shrink-0 rounded-full bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">
            {entry.projectName}
          </span>
        ) : null}
      </button>
    </li>
  );
}

function DividerRow({
  divider,
  onRemove,
  refCallback,
}: {
  divider: Divider;
  onRemove: (id: string) => void;
  refCallback?: RefCallback<HTMLLIElement>;
}) {
  return (
    <li ref={refCallback} className="group relative bg-muted/20">
      <div className="flex items-center gap-2 px-3 py-1.5 text-[10px] font-medium tracking-wide text-muted-foreground uppercase">
        <span className="h-px flex-1 bg-border" />
        <span className="font-mono tabular-nums">{formatClock(new Date(divider.at))}</span>
        <span className="h-px flex-1 bg-border" />
        <button
          type="button"
          onClick={() => onRemove(divider.id)}
          className="absolute top-1/2 right-1 -translate-y-1/2 rounded p-0.5 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:bg-muted hover:text-foreground"
          aria-label="Remove marker"
        >
          <XIcon className="size-3" />
        </button>
      </div>
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
