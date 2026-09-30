"use client";

import type { LaunchSessionDto } from "@/types/localify";

import { DangerTriangleIcon } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

import { describeLaunchSession, hasLaunchSessionBadge } from "./util.launch-profiles";

interface LaunchProfileBadgeProps {
  session: LaunchSessionDto;
  className?: string;
}

export const LaunchProfileBadge = ({ session, className }: LaunchProfileBadgeProps) => {
  if (!hasLaunchSessionBadge(session)) {
    return null;
  }

  const description = describeLaunchSession(session);

  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <span
            className={cn(
              "inline-flex h-5 max-w-28 shrink-0 items-center gap-1 rounded-full border px-1.5 text-[11px] font-medium",
              session.danger
                ? "border-destructive-foreground/30 bg-destructive-foreground/10 text-destructive-foreground"
                : "border-border bg-muted text-muted-foreground",
              className,
            )}
          />
        }
      >
        {session.danger ? <DangerTriangleIcon aria-hidden className="size-3 shrink-0" /> : null}
        <span aria-hidden className="truncate">
          {session.profile}
        </span>
        <span className="sr-only">{description}</span>
      </TooltipTrigger>
      <TooltipContent>{description}</TooltipContent>
    </Tooltip>
  );
};
