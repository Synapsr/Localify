"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type MouseEvent, type PointerEvent } from "react";
import { toast } from "sonner";

import type { DomainInfoDto } from "@/types/roxy";

import {
  CheckIcon,
  CopyIcon,
  DotsIcon,
  EditIcon,
  ExternalLinkIcon,
  PlayIcon,
  SpinnerIcon,
  TrashIcon,
  XIcon,
} from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { bootstrapQueries, projectMutations } from "@/lib/queries";
import { tauriClient } from "@/lib/tauri-client";
import { cn } from "@/lib/utils";

import { ProjectDeleteDomainDialog } from "./projects-list-item-delete-domain-dialog";
import { ProjectEditDomainDialog } from "./projects-list-item-edit-domain-dialog";

interface ProjectDomainChipProps {
  domain: DomainInfoDto;
  projectRootPath: string | null;
}

function portSegment(
  scheme: "http" | "https",
  port: number | undefined,
): string {
  if (!port) return "";
  if (scheme === "http" && port === 80) return "";
  if (scheme === "https" && port === 443) return "";
  return `:${port}`;
}

function buildUrl(
  domain: DomainInfoDto,
  ports: { httpPort: number; httpsPort: number } | undefined,
): string {
  const scheme = domain.registration.httpsEnabled ? "https" : "http";
  const port = domain.registration.httpsEnabled
    ? ports?.httpsPort
    : ports?.httpPort;
  return `${scheme}://${domain.registration.domain}${portSegment(scheme, port)}`;
}

function extractPort(target: string | undefined): string | null {
  if (!target) return null;
  try {
    const parsed = new URL(target);
    if (parsed.port) return parsed.port;
    if (parsed.protocol === "https:") return "443";
    if (parsed.protocol === "http:") return "80";
    return null;
  } catch {
    const match = target.match(/:(\d+)(?:\/|$)/);
    return match ? match[1] : null;
  }
}

function getPrimaryLaunchTarget(domain: DomainInfoDto): string | null {
  const rootRoute = domain.registration.routes.find(
    (route) => route.path === "/",
  );
  const candidate = rootRoute ?? domain.registration.routes[0];
  return candidate?.target ?? null;
}

export const ProjectDomainChip = ({
  domain,
  projectRootPath,
}: ProjectDomainChipProps) => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const [editDialogOpen, setEditDialogOpen] = useState(false);
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const { copy: copyUrl, copied: urlCopied } = useCopyToClipboard();
  const { copy: copyPort, copied: portCopied } = useCopyToClipboard();
  const runLocalDevServer = useMutation(
    projectMutations.runLocalDevServer(queryClient),
  );
  const stopLocalDevServer = useMutation(
    projectMutations.stopLocalDevServer(queryClient),
  );
  const { pattern, routes } = domain.registration;
  const url = buildUrl(domain, data?.config.daemon);
  const reachable = domain.targetReachable;
  const port = extractPort(routes[0]?.target);
  const launchTarget = getPrimaryLaunchTarget(domain);
  const launchPort = extractPort(launchTarget ?? undefined);
  const canRunLocalDevServer = Boolean(
    projectRootPath && launchTarget && launchPort,
  );
  const canStopLocalDevServer = Boolean(
    launchTarget && launchPort && reachable,
  );
  const localDevServerPending =
    runLocalDevServer.isPending || stopLocalDevServer.isPending;
  const localDevServerUnavailable = reachable
    ? !canStopLocalDevServer
    : !canRunLocalDevServer;
  const localDevServerDisabled =
    localDevServerPending || localDevServerUnavailable;
  const localDevServerLabel = reachable
    ? "Stop local dev server"
    : "Run local dev server";
  const localDevServerMenuLabel = localDevServerPending
    ? reachable
      ? "Stopping local dev server"
      : "Starting local dev server"
    : localDevServerUnavailable
      ? reachable
        ? "No running local dev server"
        : "No linked local app"
      : localDevServerLabel;

  const stopPointerPropagation = (event: PointerEvent) => {
    event.stopPropagation();
  };

  const handleCopyUrl = async () => {
    await copyUrl(url);
  };

  const handleCopyPort = async (event: MouseEvent) => {
    event.stopPropagation();
    event.preventDefault();
    if (!port) return;
    await copyPort(port);
  };

  const handleOpen = async () => {
    try {
      await tauriClient.openProjectUrl({
        url,
        browser: data?.launcherPreferences.browser ?? "default",
      });
    } catch (error) {
      const description =
        error instanceof Error ? error.message : "Unable to open this URL.";
      toast.error("Open in browser failed", { description });
    }
  };

  const handleOpenLinkClick = (event: MouseEvent) => {
    event.stopPropagation();
    event.preventDefault();
    void handleOpen();
  };

  const handleToggleLocalDevServer = async () => {
    try {
      if (reachable) {
        if (!launchTarget) {
          return;
        }

        await stopLocalDevServer.mutateAsync({
          target: launchTarget,
        });
        return;
      }

      if (!projectRootPath || !launchTarget) {
        return;
      }

      await runLocalDevServer.mutateAsync({
        rootPath: projectRootPath,
        target: launchTarget,
      });
    } catch (error) {
      const description =
        error instanceof Error
          ? error.message
          : "Unable to update the local dev server.";
      toast.error(`${localDevServerLabel} failed`, { description });
    }
  };

  return (
    <>
      <span
        onPointerDown={stopPointerPropagation}
        className={cn(
          "group/chip inline-flex h-8 items-center gap-1 rounded-full border border-border pr-1 pl-2 text-sm text-foreground transition-colors",
          "hover:border-foreground/20 hover:bg-muted/50",
          !reachable ? "opacity-60" : "opacity-80",
        )}
      >
        <Tooltip>
          <TooltipTrigger
            render={
              <span
                className={cn(
                  "size-4 shrink-0 rounded-full",
                  reachable ? "bg-success" : "bg-neutral-400",
                )}
              />
            }
          />
          <TooltipContent>
            {reachable ? "Target reachable" : "Target offline"}
          </TooltipContent>
        </Tooltip>

        <Tooltip>
          <TooltipTrigger
            render={
              <a
                href={url}
                target="_blank"
                rel="noreferrer"
                onClick={handleOpenLinkClick}
                className="inline-flex h-full items-center rounded-full px-1 hover:text-foreground"
              />
            }
          >
            <span className="truncate">{pattern}</span>
          </TooltipTrigger>
          <TooltipContent>Open {url}</TooltipContent>
        </Tooltip>

        {port && (
          <Tooltip>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  onClick={handleCopyPort}
                  aria-label={portCopied ? "Port copied" : `Copy port ${port}`}
                  className="inline-flex h-5 shrink-0 items-center gap-0.5 rounded bg-muted px-1 text-xs text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground"
                />
              }
            >
              {portCopied ? (
                <CheckIcon className="size-3 text-emerald-500" />
              ) : (
                <span>:{port}</span>
              )}
            </TooltipTrigger>
            <TooltipContent>
              {portCopied ? "Port copied" : `Copy port ${port}`}
            </TooltipContent>
          </Tooltip>
        )}

        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                type="button"
                size="icon-xs"
                variant="ghost"
                aria-label={`Domain actions for ${pattern}`}
                className="ml-0.5 size-6 rounded-full text-muted-foreground hover:bg-foreground/10 hover:text-foreground aria-expanded:bg-foreground/10 aria-expanded:text-foreground"
              >
                <DotsIcon className="size-5 stroke-2" />
              </Button>
            }
          />
          <DropdownMenuContent align="end" sideOffset={6} className="w-56">
            <DropdownMenuItem
              onClick={(event) => {
                event.stopPropagation();
                void handleOpen();
              }}
            >
              <ExternalLinkIcon />
              Open in browser
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={(event) => {
                event.stopPropagation();
                void handleCopyUrl();
              }}
            >
              {urlCopied ? (
                <CheckIcon className="text-emerald-500" />
              ) : (
                <CopyIcon />
              )}
              {urlCopied ? "URL copied" : "Copy URL"}
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={(event) => {
                event.stopPropagation();
                void handleToggleLocalDevServer();
              }}
              disabled={localDevServerDisabled}
            >
              {localDevServerPending ? (
                <SpinnerIcon className="animate-spin" />
              ) : reachable ? (
                <XIcon />
              ) : (
                <PlayIcon className="translate-x-px" />
              )}
              {localDevServerMenuLabel}
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={(event) => {
                event.stopPropagation();
                setEditDialogOpen(true);
              }}
            >
              <EditIcon />
              Edit domain
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              variant="destructive"
              onClick={(event) => {
                event.stopPropagation();
                setDeleteDialogOpen(true);
              }}
            >
              <TrashIcon />
              Remove domain
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </span>

      <ProjectEditDomainDialog
        domain={domain}
        open={editDialogOpen}
        onOpenChange={setEditDialogOpen}
      />
      <ProjectDeleteDomainDialog
        domain={domain}
        open={deleteDialogOpen}
        onOpenChange={setDeleteDialogOpen}
      />
    </>
  );
};
