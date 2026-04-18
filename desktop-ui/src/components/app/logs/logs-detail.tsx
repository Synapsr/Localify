"use client";

import { XIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

import type { RequestEntry } from "./request-parser";

interface LogsDetailProps {
  entry: RequestEntry | null;
  onClose?: () => void;
}

export const LogsDetail = ({ entry, onClose }: LogsDetailProps) => {
  if (!entry) return null;

  const tone = statusColor(entry.status);
  const kindLabel = kindDisplay(entry.kind);

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-start justify-between gap-3 border-b border-border px-6 pt-6 pb-4">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-3 font-mono">
            {entry.method ? (
              <span className="text-sm font-semibold tracking-tight text-foreground">
                {entry.method}
              </span>
            ) : null}
            <span className="truncate text-lg font-semibold tracking-tight text-foreground">
              {entry.path ?? "/"}
            </span>
          </div>
          <p className="mt-1 font-mono text-xs text-muted-foreground">{entry.host}</p>
        </div>
        {onClose ? (
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={onClose}
            aria-label="Close detail pane"
          >
            <XIcon />
          </Button>
        ) : null}
      </div>

      <div className="flex-1 space-y-6 overflow-auto px-6 py-5">
        <section>
          <SectionTitle>Details</SectionTitle>
          <dl className="divide-y divide-border overflow-hidden rounded-md border border-border">
            <DetailRow label="Status">
              <span
                className={cn(
                  "inline-flex h-[20px] min-w-[44px] items-center justify-center rounded border px-1.5 font-mono text-[10px] font-semibold tabular-nums",
                  tone.badge,
                )}
              >
                {entry.status}
              </span>
              {kindLabel ? (
                <span className="ml-2 text-xs text-muted-foreground">{kindLabel}</span>
              ) : null}
            </DetailRow>
            <DetailRow label="Method">
              <span className="font-mono text-xs">{entry.method ?? "—"}</span>
            </DetailRow>
            <DetailRow label="Host">
              <span className="font-mono text-xs">{entry.host}</span>
            </DetailRow>
            <DetailRow label="Path">
              <span className="font-mono text-xs">{entry.path ?? "—"}</span>
            </DetailRow>
            <DetailRow label="Project">
              {entry.projectName ? (
                <span className="text-xs">{entry.projectName}</span>
              ) : (
                <span className="text-xs text-muted-foreground">—</span>
              )}
            </DetailRow>
            <DetailRow label="Time">
              <span className="font-mono text-xs tabular-nums">
                {entry.timestamp.toLocaleString()}
              </span>
            </DetailRow>
          </dl>
        </section>

        <section>
          <SectionTitle>Raw log line</SectionTitle>
          <pre className="overflow-x-auto rounded-md border border-border bg-muted/30 p-3 font-mono text-[11px] leading-5 whitespace-pre-wrap text-foreground">
            {entry.raw}
          </pre>
        </section>
      </div>
    </div>
  );
};

function SectionTitle({ children }: { children: React.ReactNode }) {
  return (
    <h3 className="mb-2 text-[11px] font-semibold tracking-wide text-muted-foreground uppercase">
      {children}
    </h3>
  );
}

function DetailRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-4 bg-background px-3 py-2">
      <dt className="w-24 shrink-0 text-xs text-muted-foreground">{label}</dt>
      <dd className="flex min-w-0 flex-1 items-center">{children}</dd>
    </div>
  );
}

function kindDisplay(kind: RequestEntry["kind"]): string | null {
  if (kind === "no-route") return "No route matched";
  if (kind === "domain-not-registered") return "Domain not registered";
  return null;
}

function statusColor(status: number): { badge: string } {
  if (status >= 500) return { badge: "bg-red-50 text-red-700 border-red-200" };
  if (status >= 400) return { badge: "bg-amber-50 text-amber-700 border-amber-200" };
  if (status >= 300) return { badge: "bg-sky-50 text-sky-700 border-sky-200" };
  if (status >= 200) return { badge: "bg-emerald-50 text-emerald-700 border-emerald-200" };
  return { badge: "bg-neutral-50 text-neutral-700 border-neutral-200" };
}
