"use client";

import { RiCheckLine, RiCloseLine, RiFileCopyLine } from "@remixicon/react";
import { type MouseEvent, type PointerEvent } from "react";

import type { DomainInfoDto } from "@/types/roxy";

import { DialogTrigger } from "@/components/ui/dialog";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { cn } from "@/lib/utils";

import { ProjectDeleteDomainDialog } from "./projects-list-item-delete-domain-dialog";

interface ProjectDomainChipProps {
  domain: DomainInfoDto;
}

function buildUrl(domain: DomainInfoDto): string {
  const scheme = domain.registration.httpsEnabled ? "https" : "http";
  return `${scheme}://${domain.registration.domain}`;
}

export const ProjectDomainChip = ({ domain }: ProjectDomainChipProps) => {
  const { copy, copied } = useCopyToClipboard();
  const { pattern } = domain.registration;
  const url = buildUrl(domain);
  const reachable = domain.targetReachable;

  const stopMousePropagation = (event: MouseEvent) => {
    event.stopPropagation();
  };

  const stopPointerPropagation = (event: PointerEvent) => {
    event.stopPropagation();
  };

  const handleCopy = async (event: MouseEvent) => {
    event.stopPropagation();
    await copy(pattern);
  };

  return (
    <span
      onPointerDown={stopPointerPropagation}
      className={cn(
        "group/chip relative inline-flex h-6 items-center rounded-full border border-border bg-background pl-0.5 font-mono text-xs text-foreground/80 transition-colors",
        "hover:border-foreground/20 hover:bg-muted",
        !reachable && "opacity-60",
      )}
    >
      <Tooltip>
        <TooltipTrigger
          render={
            <div
              className={cn(
                "ml-1 size-3 rounded-full bg-neutral-300",
                reachable ? "bg-success" : "bg-neutral-300",
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
              onClick={stopMousePropagation}
              className="inline-flex h-full items-center rounded-full px-1.5 hover:text-foreground"
            />
          }
        >
          <span className="max-w-[22ch] truncate">{pattern}</span>
        </TooltipTrigger>
        <TooltipContent>Open {url}</TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger
          render={
            <button
              type="button"
              onClick={handleCopy}
              aria-label={copied ? "Copied" : `Copy ${pattern}`}
              className="mr-0.5 inline-flex size-5 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground"
            />
          }
        >
          {copied ? (
            <RiCheckLine className="size-3 text-emerald-500" />
          ) : (
            <RiFileCopyLine className="size-3" />
          )}
        </TooltipTrigger>
        <TooltipContent>{copied ? "Copied" : "Copy"}</TooltipContent>
      </Tooltip>
      <ProjectDeleteDomainDialog domain={domain}>
        <Tooltip>
          <TooltipTrigger
            render={
              <DialogTrigger
                type="button"
                aria-label={`Remove ${pattern}`}
                onClick={stopMousePropagation}
                className="pointer-events-none absolute top-1/2 right-[22px] inline-flex size-5 -translate-y-1/2 items-center justify-center rounded-full bg-foreground/10 text-muted-foreground opacity-0 transition-[opacity,background-color,color] group-hover/chip:pointer-events-auto group-hover/chip:opacity-100 hover:bg-destructive/10 hover:text-destructive focus-visible:pointer-events-auto focus-visible:opacity-100 data-popup-open:pointer-events-auto data-popup-open:opacity-100"
              />
            }
          >
            <RiCloseLine className="size-3" />
          </TooltipTrigger>
          <TooltipContent>Remove domain</TooltipContent>
        </Tooltip>
      </ProjectDeleteDomainDialog>
    </span>
  );
};
