import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import type { DomainInfoDto, ProjectInfoDto } from "../types/localify";

import { cn } from "../lib/cn";
import { projectMutations } from "../lib/queries";
import { Badge } from "./ui/badge";
import { Card } from "./ui/card";

const PREVIEW_LIMIT = 3;

export type DragState = "idle" | "dragging" | "over";

interface ProjectRowProps {
  project: ProjectInfoDto;
  domains: DomainInfoDto[];
  draggable: boolean;
  dragState: DragState;
  highlight: string;
  onAddDomain: () => void;
  onDragStart: (event: React.DragEvent<HTMLDivElement>) => void;
  onDragEnter: (event: React.DragEvent<HTMLDivElement>) => void;
  onDragOver: (event: React.DragEvent<HTMLDivElement>) => void;
  onDragLeave: (event: React.DragEvent<HTMLDivElement>) => void;
  onDrop: (event: React.DragEvent<HTMLDivElement>) => void;
  onDragEnd: (event: React.DragEvent<HTMLDivElement>) => void;
}

export function ProjectRow({
  project,
  domains,
  draggable,
  dragState,
  highlight,
  onAddDomain,
  onDragStart,
  onDragEnter,
  onDragOver,
  onDragLeave,
  onDrop,
  onDragEnd,
}: ProjectRowProps) {
  const queryClient = useQueryClient();
  const updateProject = useMutation(projectMutations.update(queryClient));
  const [expanded, setExpanded] = useState(false);

  async function handleToggle() {
    await updateProject.mutateAsync({
      id: project.id,
      enabled: !project.enabled,
    });
  }

  const visibleDomains = expanded ? domains : domains.slice(0, PREVIEW_LIMIT);
  const hiddenCount = Math.max(0, domains.length - visibleDomains.length);
  const matchesHighlight = highlight.trim().toLowerCase();

  return (
    <Card
      onDragEnter={onDragEnter}
      onDragOver={onDragOver}
      onDragLeave={onDragLeave}
      onDrop={onDrop}
      className={cn(
        "overflow-hidden transition-[transform,box-shadow,border-color]",
        dragState === "dragging" && "opacity-50",
        dragState === "over" && "border-[--color-accent] shadow-[0_0_0_2px_var(--color-accent)]",
      )}
    >
      <div className="flex items-start gap-3 border-b border-[--color-border] p-4">
        <div
          draggable={draggable}
          onDragStart={onDragStart}
          onDragEnd={onDragEnd}
          aria-label={draggable ? "Drag to reorder" : undefined}
          title={draggable ? "Drag to reorder" : "Switch sort to Custom to reorder"}
          className={cn(
            "mt-1 flex h-6 w-4 shrink-0 flex-col items-center justify-center rounded",
            draggable
              ? "cursor-grab text-[--color-muted] hover:text-[--color-ink] active:cursor-grabbing"
              : "cursor-not-allowed text-[--color-border]",
          )}
        >
          <span aria-hidden className="text-sm leading-none">
            ⋮⋮
          </span>
        </div>

        <div className="min-w-0 flex-1 space-y-2">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="truncate text-base font-semibold text-[--color-ink]">
              <HighlightedText text={project.name} query={matchesHighlight} />
            </h2>
            <Badge tone={project.enabled ? "accent" : "muted"}>
              {project.enabled ? "Enabled" : "Disabled"}
            </Badge>
            <Badge tone="muted">
              {project.domainCount} domain{project.domainCount === 1 ? "" : "s"}
            </Badge>
          </div>
          {domains.length === 0 ? (
            <p className="text-xs text-[--color-muted]">
              No domains yet — add one to route traffic through this project.
            </p>
          ) : (
            <p className="text-xs text-[--color-muted]">
              {expanded
                ? "Showing every domain in this project."
                : `Preview of ${visibleDomains.length} of ${domains.length} domain${domains.length === 1 ? "" : "s"}.`}
            </p>
          )}
        </div>

        <div className="flex shrink-0 items-center gap-1.5">
          <RowButton onClick={onAddDomain}>+ Domain</RowButton>
          <RowButton onClick={handleToggle} disabled={updateProject.isPending}>
            {project.enabled ? "Disable" : "Enable"}
          </RowButton>
        </div>
      </div>

      {domains.length > 0 && (
        <ul className="divide-y divide-[--color-border]">
          {visibleDomains.map((domain) => (
            <DomainPreview
              key={domain.registration.pattern}
              domain={domain}
              expanded={expanded}
              highlight={matchesHighlight}
            />
          ))}
        </ul>
      )}

      {domains.length > PREVIEW_LIMIT && (
        <button
          type="button"
          onClick={() => setExpanded((value) => !value)}
          className="flex w-full items-center justify-center gap-1.5 border-t border-[--color-border] bg-[--color-bg]/40 px-4 py-2 text-xs text-[--color-muted] hover:text-[--color-ink]"
        >
          {expanded ? (
            <>Collapse</>
          ) : (
            <>
              Show {hiddenCount} more domain{hiddenCount === 1 ? "" : "s"}
            </>
          )}
        </button>
      )}
    </Card>
  );
}

interface DomainPreviewProps {
  domain: DomainInfoDto;
  expanded: boolean;
  highlight: string;
}

function DomainPreview({ domain, expanded, highlight }: DomainPreviewProps) {
  const { registration } = domain;
  const primaryRoute = registration.routes[0];
  const extraRoutes = registration.routes.slice(1);

  return (
    <li className="px-4 py-3">
      <div className="flex items-center gap-3">
        <span className="truncate font-mono text-xs font-medium text-[--color-ink]">
          <HighlightedText text={registration.pattern} query={highlight} />
        </span>
        {registration.httpsEnabled && <Badge tone="accent">HTTPS</Badge>}
        {registration.wildcard && <Badge tone="muted">Wildcard</Badge>}
        {!domain.active && <Badge tone="muted">Inactive</Badge>}
        <span className="flex-1" />
        {primaryRoute && (
          <span className="truncate font-mono text-xs text-[--color-muted]">
            {primaryRoute.path === "/" ? "" : `${primaryRoute.path} `}→ {primaryRoute.target}
          </span>
        )}
      </div>

      {expanded && extraRoutes.length > 0 && (
        <ul className="mt-2 space-y-1 pl-4">
          {extraRoutes.map((route) => (
            <li
              key={route.path}
              className="flex items-center gap-2 font-mono text-xs text-[--color-muted]"
            >
              <span>{route.path}</span>
              <span>→</span>
              <span className="truncate text-[--color-ink]">{route.target}</span>
            </li>
          ))}
        </ul>
      )}
    </li>
  );
}

interface RowButtonProps {
  children: React.ReactNode;
  onClick: () => void;
  disabled?: boolean;
}

function RowButton({ children, onClick, disabled }: RowButtonProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className="inline-flex h-7 items-center rounded-md border border-[--color-border] bg-[--color-surface] px-2.5 text-xs font-medium text-[--color-ink] transition-colors hover:border-[--color-border] disabled:cursor-not-allowed disabled:opacity-50"
    >
      {children}
    </button>
  );
}

interface HighlightedTextProps {
  text: string;
  query: string;
}

function HighlightedText({ text, query }: HighlightedTextProps) {
  if (!query) return <>{text}</>;
  const lower = text.toLowerCase();
  const index = lower.indexOf(query);
  if (index === -1) return <>{text}</>;
  return (
    <>
      {text.slice(0, index)}
      <mark className="rounded bg-[--color-accent]/20 px-0.5 text-[--color-ink]">
        {text.slice(index, index + query.length)}
      </mark>
      {text.slice(index + query.length)}
    </>
  );
}
