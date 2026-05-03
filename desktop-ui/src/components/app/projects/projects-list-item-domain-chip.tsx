"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { motion } from "motion/react";
import { type MouseEvent, type PointerEvent } from "react";
import { toast } from "sonner";

import type { DomainInfoDto } from "@/types/roxy";

import {
  CheckIcon,
  CopyIcon,
  EditIcon,
  ExternalLinkIcon,
  PlayIcon,
  SpinnerIcon,
  TrashIcon,
  XIcon,
} from "@/components/icons";
import { DialogTrigger } from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
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

const actionVariants = {
  rest: { opacity: 0, scale: 0.6, x: -6 },
  hover: { opacity: 1, scale: 1, x: 0 },
};

function portSegment(scheme: "http" | "https", port: number | undefined): string {
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
  const port = domain.registration.httpsEnabled ? ports?.httpsPort : ports?.httpPort;
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
  const rootRoute = domain.registration.routes.find((route) => route.path === "/");
  const candidate = rootRoute ?? domain.registration.routes[0];
  return candidate?.target ?? null;
}

export const ProjectDomainChip = ({ domain, projectRootPath }: ProjectDomainChipProps) => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const { copy: copyUrl, copied: urlCopied } = useCopyToClipboard();
  const { copy: copyPort, copied: portCopied } = useCopyToClipboard();
  const runLocalDevServer = useMutation(projectMutations.runLocalDevServer(queryClient));
  const stopLocalDevServer = useMutation(projectMutations.stopLocalDevServer(queryClient));
  const { pattern, routes } = domain.registration;
  const url = buildUrl(domain, data?.config.daemon);
  const reachable = domain.targetReachable;
  const port = extractPort(routes[0]?.target);
  const launchTarget = getPrimaryLaunchTarget(domain);
  const launchPort = extractPort(launchTarget ?? undefined);
  const canRunLocalDevServer = Boolean(projectRootPath && launchTarget && launchPort);
  const canStopLocalDevServer = Boolean(launchTarget && launchPort && reachable);
  const localDevServerPending = runLocalDevServer.isPending || stopLocalDevServer.isPending;
  const localDevServerError =
    stopLocalDevServer.error?.message ?? runLocalDevServer.error?.message;

  const stopMousePropagation = (event: MouseEvent) => {
    event.stopPropagation();
  };

  const stopPointerPropagation = (event: PointerEvent) => {
    event.stopPropagation();
  };

  const handleCopyUrl = async (event: MouseEvent) => {
    event.stopPropagation();
    event.preventDefault();
    await copyUrl(url);
  };

  const handleCopyPort = async (event: MouseEvent) => {
    event.stopPropagation();
    event.preventDefault();
    if (!port) return;
    await copyPort(port);
  };

  const handleOpen = async (event: MouseEvent) => {
    event.stopPropagation();
    event.preventDefault();

    try {
      await tauriClient.openProjectUrl({
        url,
        browser: data?.launcherPreferences.browser ?? "default",
      });
    } catch (error) {
      const description = error instanceof Error ? error.message : "Unable to open this URL.";
      toast.error("Open in browser failed", { description });
    }
  };

  const handleToggleLocalDevServer = async (event: MouseEvent) => {
    event.stopPropagation();
    event.preventDefault();

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
    } catch {
      // The mutation already exposes the actionable error in the tooltip.
    }
  };

  return (
    <motion.span
      onPointerDown={stopPointerPropagation}
      initial="rest"
      animate="rest"
      whileHover="hover"
      whileFocus="hover"
      className={cn(
        "group/chip inline-flex h-8 items-center gap-1 rounded-full border border-border  pr-1 pl-2  text-sm text-foreground transition-colors",
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
        <TooltipContent>{reachable ? "Target reachable" : "Target offline"}</TooltipContent>
      </Tooltip>

      <Tooltip>
        <TooltipTrigger
          render={
            <a
              href={url}
              target="_blank"
              rel="noreferrer"
              onClick={handleOpen}
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
            {portCopied ? <CheckIcon className="size-3 text-emerald-500" /> : <span>:{port}</span>}
          </TooltipTrigger>
          <TooltipContent>{portCopied ? "Port copied" : `Copy port ${port}`}</TooltipContent>
        </Tooltip>
      )}

      <motion.span
        variants={{
          rest: { width: 0, opacity: 0 },
          hover: { width: "auto", opacity: 1 },
        }}
        transition={{ type: "spring", stiffness: 420, damping: 34, mass: 0.6 }}
        className="flex items-center gap-1 overflow-hidden"
      >
        <span className="mx-0.5 h-4 w-px shrink-0 bg-border" aria-hidden />

        <motion.span
          variants={actionVariants}
          transition={{ type: "spring", stiffness: 500, damping: 30 }}
          className="inline-flex shrink-0"
        >
          <Tooltip>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  onClick={handleCopyUrl}
                  aria-label={urlCopied ? "URL copied" : `Copy ${url}`}
                  className="inline-flex size-6 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground"
                />
              }
            >
              {urlCopied ? (
                <CheckIcon className="size-3.5 text-emerald-500" />
              ) : (
                <CopyIcon className="size-3.5" />
              )}
            </TooltipTrigger>
            <TooltipContent>{urlCopied ? "URL copied" : "Copy URL"}</TooltipContent>
          </Tooltip>
        </motion.span>

        <motion.span
          variants={actionVariants}
          transition={{ type: "spring", stiffness: 500, damping: 30, delay: 0.02 }}
          className="inline-flex shrink-0"
        >
          <Tooltip>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  onClick={handleToggleLocalDevServer}
                  aria-label={`${reachable ? "Stop" : "Run"} local dev server for ${pattern}`}
                  disabled={
                    reachable ? !canStopLocalDevServer || localDevServerPending : !canRunLocalDevServer || localDevServerPending
                  }
                  className={cn(
                    "inline-flex size-6 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors disabled:opacity-40",
                    reachable
                      ? "hover:bg-destructive/10 hover:text-destructive"
                      : "hover:bg-foreground/10 hover:text-foreground",
                  )}
                />
              }
            >
              {localDevServerPending ? (
                <SpinnerIcon className="size-3.5 animate-spin" />
              ) : reachable ? (
                <XIcon className="size-3.5" />
              ) : (
                <PlayIcon className="size-4.5 translate-x-px" />
              )}
            </TooltipTrigger>
            <TooltipContent>
              {localDevServerError ??
                (reachable
                  ? canStopLocalDevServer
                    ? "Stop local dev server"
                    : "No running local dev server to stop"
                  : canRunLocalDevServer
                    ? "Run local dev server"
                    : "No linked local app to run")}
            </TooltipContent>
          </Tooltip>
        </motion.span>

        <motion.span
          variants={actionVariants}
          transition={{ type: "spring", stiffness: 500, damping: 30, delay: 0.04 }}
          className="inline-flex shrink-0"
        >
          <ProjectEditDomainDialog domain={domain}>
            <Tooltip>
              <TooltipTrigger
                render={
                  <DialogTrigger
                    type="button"
                    aria-label={`Edit ${pattern}`}
                    onClick={stopMousePropagation}
                    className="inline-flex size-6 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground data-popup-open:bg-foreground/10 data-popup-open:text-foreground"
                  />
                }
              >
                <EditIcon className="size-3.5" />
              </TooltipTrigger>
              <TooltipContent>Edit domain</TooltipContent>
            </Tooltip>
          </ProjectEditDomainDialog>
        </motion.span>

        <motion.span
          variants={actionVariants}
          transition={{ type: "spring", stiffness: 500, damping: 30, delay: 0.06 }}
          className="inline-flex shrink-0"
        >
          <Tooltip>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  onClick={handleOpen}
                  aria-label={`Open ${url}`}
                  className="inline-flex size-6 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground"
                />
              }
            >
              <ExternalLinkIcon className="size-3.5" />
            </TooltipTrigger>
            <TooltipContent>Open in browser</TooltipContent>
          </Tooltip>
        </motion.span>

        <motion.span
          variants={actionVariants}
          transition={{ type: "spring", stiffness: 500, damping: 30, delay: 0.08 }}
          className="inline-flex shrink-0"
        >
          <ProjectDeleteDomainDialog domain={domain}>
            <Tooltip>
              <TooltipTrigger
                render={
                  <DialogTrigger
                    type="button"
                    aria-label={`Remove ${pattern}`}
                    onClick={stopMousePropagation}
                    className="inline-flex size-6 shrink-0 items-center justify-center rounded-full text-muted-foreground/70 transition-colors hover:bg-destructive/10 hover:text-destructive focus-visible:bg-destructive/10 focus-visible:text-destructive data-popup-open:bg-destructive/10 data-popup-open:text-destructive"
                  />
                }
              >
                <TrashIcon className="size-3.5" />
              </TooltipTrigger>
              <TooltipContent>Remove domain</TooltipContent>
            </Tooltip>
          </ProjectDeleteDomainDialog>
        </motion.span>
      </motion.span>
    </motion.span>
  );
};
