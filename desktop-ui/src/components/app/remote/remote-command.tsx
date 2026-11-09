"use client";

import { CheckIcon, CopyIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";

/** A command to run on the target, with a copy button. */
export function RemoteCommand({ command }: { command: string }) {
  const { copy, copied } = useCopyToClipboard();
  return (
    <div className="flex items-center gap-1 rounded-md bg-muted px-2 py-1">
      <code className="min-w-0 flex-1 font-mono text-xs break-all text-foreground select-all">
        {command}
      </code>
      <Button
        size="icon-xs"
        variant="ghost"
        aria-label={copied ? "Command copied" : "Copy the command"}
        onClick={() => void copy(command)}
      >
        {copied ? <CheckIcon className="text-emerald-500" /> : <CopyIcon />}
      </Button>
    </div>
  );
}
