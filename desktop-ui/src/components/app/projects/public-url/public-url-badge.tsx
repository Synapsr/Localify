"use client";

import { GlobeIcon } from "@/components/icons";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

interface PublicUrlBadgeProps {
  /** Where it opens publicly. */
  url: string;
  onOpen: () => void;
}

/** The globe shown next to something a public URL reaches; opens its dialog. */
export const PublicUrlBadge = ({ url, onOpen }: PublicUrlBadgeProps) => (
  <Tooltip>
    <TooltipTrigger
      render={
        <button
          type="button"
          onClick={(event) => {
            event.stopPropagation();
            onOpen();
          }}
          aria-label={`Public URL ${url}`}
          className="inline-flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-foreground/10 hover:text-foreground"
        />
      }
    >
      <GlobeIcon className="size-3.5" />
    </TooltipTrigger>
    <TooltipContent>Public at {url}</TooltipContent>
  </Tooltip>
);
