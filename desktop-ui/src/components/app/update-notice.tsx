import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { XIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { bootstrapQueries, systemQueries } from "@/lib/queries";

import { RestartToUpdate } from "./restart-to-update";

/** A quiet line while an update waits for the person to restart. Hidden for this run once closed. */
export const UpdateNotice = () => {
  const { data: bootstrap } = useQuery(bootstrapQueries.read());
  const { data: status } = useQuery(systemQueries.updateStatus());
  const [dismissed, setDismissed] = useState<string | null>(null);

  const version = status?.available;
  if (!bootstrap || !version || status.state !== "available" || !status.canInstall) return null;
  if (dismissed === version) return null;

  const appName = bootstrap.environment.appName;

  return (
    <div className="flex items-center justify-between gap-4 rounded-t-lg border-b border-border/60 bg-muted/40 px-4 py-1.5">
      <p className="min-w-0 truncate text-xs text-muted-foreground">
        {appName} {version} is ready to install.
      </p>
      <div className="flex shrink-0 items-center gap-1">
        <RestartToUpdate appName={appName} version={version} size="xs" />
        <Button
          size="icon-xs"
          variant="ghost"
          aria-label="Hide until next launch"
          onClick={() => setDismissed(version)}
        >
          <XIcon />
        </Button>
      </div>
    </div>
  );
};
