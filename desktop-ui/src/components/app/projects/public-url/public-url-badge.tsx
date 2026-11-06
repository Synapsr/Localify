"use client";

import type { PublicUrlDto } from "@/types/localify";

import { GlobeIcon } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

interface PublicUrlBadgeProps {
  publicUrl: PublicUrlDto;
  onOpen: () => void;
}

/** The globe shown next to something that has a public URL; opens its dialog. */
export const PublicUrlBadge = ({ publicUrl, onOpen }: PublicUrlBadgeProps) => (
  <Tooltip>
    <TooltipTrigger
      render={
        <button
          type="button"
          onClick={(event) => {
            event.stopPropagation();
            onOpen();
          }}
          aria-label={`Public URL ${publicUrl.url}`}
          className="inline-flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground"
        />
      }
    >
      <GlobeIcon className="size-3.5" />
    </TooltipTrigger>
    <TooltipContent>Public at {publicUrl.url}</TooltipContent>
  </Tooltip>
);
