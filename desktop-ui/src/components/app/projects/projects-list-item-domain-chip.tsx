"use client";

import { openUrl } from "@tauri-apps/plugin-opener";
import { useMutation } from "@tanstack/react-query";
import { motion } from "motion/react";
import { type MouseEvent, type PointerEvent } from "react";

import type { DomainInfoDto } from "@/types/roxy";

import {
  CheckIcon,
  CopyIcon,
  ExternalLinkIcon,
  PlayIcon,
  SpinnerIcon,
  TrashIcon,
} from "@/components/icons";
import { DialogTrigger } from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { projectMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { ProjectDeleteDomainDialog } from "./projects-list-item-delete-domain-dialog";

interface ProjectDomainChipProps {
  domain: DomainInfoDto;
  projectRootPath: string | null;
}

const actionVariants = {
  rest: { opacity: 0, scale: 0.6, x: -6 },
  hover: { opacity: 1, scale: 1, x: 0 },
};

function buildUrl(domain: DomainInfoDto): string {
  const scheme = domain.registration.httpsEnabled ? "https" : "http";
  return `${scheme}://${domain.registration.domain}`;
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
  const { copy: copyUrl, copied: urlCopied } = useCopyToClipboard();
  const { copy: copyPort, copied: portCopied } = useCopyToClipboard();
  const runLocalDevServer = useMutation(projectMutations.runLocalDevServer());
  const { pattern, routes } = domain.registration;
  const url = buildUrl(domain);
  const reachable = domain.targetReachable;
  const port = extractPort(routes[0]?.target);
  const launchTarget = getPrimaryLaunchTarget(domain);
  const canRunLocalDevServer = Boolean(projectRootPath && launchTarget);

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

  const handleOpen = (event: MouseEvent) => {
    event.stopPropagation();
    event.preventDefault();
    void openUrl(url);
  };

  const handleRunLocalDevServer = async (event: MouseEvent) => {
    event.stopPropagation();
    event.preventDefault();
    if (!projectRootPath || !launchTarget) {
      return;
    }

    try {
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
                  onClick={handleRunLocalDevServer}
                  aria-label={`Run local dev server for ${pattern}`}
                  disabled={!canRunLocalDevServer || runLocalDevServer.isPending}
                  className="inline-flex size-6 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground disabled:opacity-40"
                />
              }
            >
              {runLocalDevServer.isPending ? (
                <SpinnerIcon className="size-3.5 animate-spin" />
              ) : (
                <PlayIcon className="size-3.5 translate-x-px" />
              )}
            </TooltipTrigger>
            <TooltipContent>
              {runLocalDevServer.error?.message ??
                (canRunLocalDevServer ? "Run local dev server" : "No linked local app to run")}
            </TooltipContent>
          </Tooltip>
        </motion.span>

        <motion.span
          variants={actionVariants}
          transition={{ type: "spring", stiffness: 500, damping: 30, delay: 0.04 }}
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
