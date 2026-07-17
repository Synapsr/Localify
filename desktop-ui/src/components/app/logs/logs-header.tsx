"use client";

import type { DomainInfoDto, ProjectInfoDto } from "@/types/localify";

import {
  BookmarkIcon,
  DotsIcon,
  PauseIcon,
  PlayIcon,
  RefreshIcon,
  TrashIcon,
} from "@/components/icons";
import { ProgressiveBlur } from "@/components/progresive-blur";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useScroll } from "@/hooks/use-scroll";

export type LogsViewMode = "requests" | "raw";
export type LogsRequestDensity = "focus" | "all";

interface LogsHeaderProps {
  projects: ProjectInfoDto[];
  projectId: string;
  onProjectChange: (projectId: string) => void;
  domains: DomainInfoDto[];
  domain: string;
  onDomainChange: (domain: string) => void;
  viewMode: LogsViewMode;
  onViewModeChange: (mode: LogsViewMode) => void;
  follow: boolean;
  onToggleFollow: () => void;
  onMark: () => void;
  onRefresh: () => void;
  isRefreshing: boolean;
  onClear: () => void;
  onShowHidden: () => void;
  canShowHidden: boolean;
  onClearDividers: () => void;
  canClearDividers: boolean;
  requestDensity: LogsRequestDensity;
  onRequestDensityChange: (density: LogsRequestDensity) => void;
  pathPrefixes: string;
  onPathPrefixesChange: (prefixes: string) => void;
  visibleRequestCount: number;
  hiddenNoiseCount: number;
  hiddenPrefixCount: number;
  issueCount: number;
}

export const LogsHeader = ({
  projects,
  projectId,
  onProjectChange,
  domains,
  domain,
  onDomainChange,
  viewMode,
  onViewModeChange,
  follow,
  onToggleFollow,
  onMark,
  onRefresh,
  isRefreshing,
  onClear,
  onShowHidden,
  canShowHidden,
  onClearDividers,
  canClearDividers,
  requestDensity,
  onRequestDensityChange,
  pathPrefixes,
  onPathPrefixesChange,
  visibleRequestCount,
  hiddenNoiseCount,
  hiddenPrefixCount,
  issueCount,
}: LogsHeaderProps) => {
  const { scrolled } = useScroll(0);

  return (
    <header className="sticky top-0 z-10 space-y-2 bg-background py-4 pt-12">
      <ProgressiveBlur
        position="top"
        height="100px"
        className={`top-0 transition-opacity duration-200 ${scrolled ? "opacity-100" : "opacity-0"}`}
      />

      <div className="relative z-10 flex items-end justify-between gap-4">
        <div className="relative z-10 min-w-0">
          <h1 className="text-2xl font-semibold tracking-tight">Logs</h1>
          <p className="mt-1 text-xs text-muted-foreground">
            {viewMode === "raw"
              ? "Raw daemon output"
              : `${visibleRequestCount} visible${issueCount > 0 ? ` · ${issueCount} issue${issueCount === 1 ? "" : "s"}` : ""}${hiddenNoiseCount > 0 ? ` · ${hiddenNoiseCount} routine asset request${hiddenNoiseCount === 1 ? "" : "s"} hidden` : ""}${hiddenPrefixCount > 0 ? ` · ${hiddenPrefixCount} prefix-filtered` : ""}`}
          </p>
        </div>

        <div className="flex flex-wrap items-center justify-end gap-2">
          {viewMode === "requests" ? (
            <select
              value={projectId}
              onChange={(e) => onProjectChange(e.target.value)}
              aria-label="Filter by project"
              className="h-8 rounded-md border border-border bg-background px-2 text-xs"
            >
              <option value="all">All projects</option>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          ) : null}

          {viewMode === "requests" && projectId !== "all" && domains.length > 0 ? (
            <select
              value={domain}
              onChange={(e) => onDomainChange(e.target.value)}
              aria-label="Filter by domain"
              className="h-8 max-w-[220px] rounded-md border border-border bg-background px-2 text-xs"
            >
              <option value="all">All domains</option>
              {domains.map((d) => (
                <option key={d.registration.pattern} value={d.registration.domain}>
                  {d.registration.domain}
                </option>
              ))}
            </select>
          ) : null}

          {viewMode === "requests" ? (
            <input
              value={pathPrefixes}
              onChange={(e) => onPathPrefixesChange(e.target.value)}
              placeholder="Hide prefixes"
              aria-label="Hide request path prefixes"
              className="h-8 w-[180px] rounded-md border border-border bg-background px-2 font-mono text-xs"
            />
          ) : null}

          {viewMode === "requests" ? (
            <Tooltip>
              <TooltipTrigger
                render={
                  <Button variant="outline" size="sm" onClick={onMark}>
                    <BookmarkIcon />
                    Marquer
                  </Button>
                }
              />
              <TooltipContent>Insert a divider at the current time</TooltipContent>
            </Tooltip>
          ) : null}

          <Tooltip>
            <TooltipTrigger
              render={
                <Button variant={follow ? "primary" : "outline"} size="sm" onClick={onToggleFollow}>
                  {follow ? <PauseIcon /> : <PlayIcon />}
                  {follow ? "Pause" : "Resume"}
                </Button>
              }
            />
            <TooltipContent>{follow ? "Pause live tail" : "Resume live tail"}</TooltipContent>
          </Tooltip>

          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button variant="outline" size="icon-sm" aria-label="More options">
                  <DotsIcon />
                </Button>
              }
            />
            <DropdownMenuContent align="end" className="w-fit">
              <DropdownMenuItem
                onClick={() => onViewModeChange(viewMode === "raw" ? "requests" : "raw")}
              >
                {viewMode === "raw" ? "Show requests" : "Show raw logs"}
              </DropdownMenuItem>
              {viewMode === "requests" ? (
                <DropdownMenuCheckboxItem
                  className="text-nowrap"
                  checked={requestDensity === "focus"}
                  onCheckedChange={(checked) => onRequestDensityChange(checked ? "focus" : "all")}
                >
                  Hide routine asset requests
                </DropdownMenuCheckboxItem>
              ) : null}
              <DropdownMenuItem onClick={onRefresh} disabled={isRefreshing}>
                <RefreshIcon className={isRefreshing ? "animate-spin" : undefined} />
                Refresh
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              {viewMode === "requests" ? (
                <>
                  <DropdownMenuItem onClick={onClear}>
                    <TrashIcon />
                    Clear visible
                  </DropdownMenuItem>
                  <DropdownMenuItem onClick={onShowHidden} disabled={!canShowHidden}>
                    Show hidden
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    onClick={onClearDividers}
                    disabled={!canClearDividers}
                    variant="destructive"
                  >
                    Clear markers
                  </DropdownMenuItem>
                </>
              ) : null}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
    </header>
  );
};
