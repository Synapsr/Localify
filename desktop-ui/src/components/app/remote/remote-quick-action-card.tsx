"use client";

import { useEffect } from "react";

import type { RemoteControlAction, RemoteInventoryItemDto } from "@/types/localify";

import { SpinnerIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import { remoteErrorMessage } from "@/lib/util.remote";
import { accessCommand, type QuickAction } from "@/lib/util.remote-controls";
import { cn } from "@/lib/utils";

import { RemoteCommand } from "./remote-command";
import { useRemoteControls } from "./remote-controls";

/**
 * Why a control is not allowed, and what fixes it (docs/remote-machines.md, "Quick actions"):
 * ask the target, run a command there, or relaunch a legacy run.
 */
export function RemoteQuickActionCard({
  item,
  action,
  quick,
  onDone,
}: {
  item: RemoteInventoryItemDto;
  action: RemoteControlAction;
  quick: Exclude<QuickAction, { kind: "run" }>;
  onDone: () => void;
}) {
  const { machineName: name, access, ask, relaunch, dismissAccess } = useRemoteControls();
  // An answer shown here is not shown again in the next card.
  useEffect(() => dismissAccess, [dismissAccess]);
  const code = item.controls[quick.kind === "relaunch" ? "restart" : action].code ?? "";

  switch (quick.kind) {
    case "message":
      return <p className="text-foreground">{quick.text}</p>;
    case "command":
      return (
        <div className="flex flex-col gap-2">
          <p className="text-foreground">{remoteErrorMessage(code, name, "Not available.")}</p>
          <p className="text-muted-foreground">Update Localify on {name} to ask from here.</p>
          <RemoteCommand command={accessCommand(quick.wants)} />
        </div>
      );
    case "ask":
      return (
        <div className="flex flex-col gap-2">
          <p className="text-foreground">{remoteErrorMessage(code, name, "Not available.")}</p>
          <AccessStatus />
          {access?.view.step === "waiting" ? null : (
            <Button
              size="sm"
              variant="primary"
              className="self-start"
              onClick={() => ask(quick.wants)}
            >
              Ask {name}
            </Button>
          )}
          <p className="mt-1 text-xs text-muted-foreground">Or run this on {name}:</p>
          <RemoteCommand command={accessCommand(quick.wants)} />
        </div>
      );
    case "relaunch":
      return (
        <div className="flex flex-col gap-2">
          <p className="text-foreground">
            {item.service.name} started before this version of Localify on {name}.
          </p>
          {quick.access ? (
            <p className="text-muted-foreground">
              {remoteErrorMessage(code, name, "")} Relaunching asks {name} for access first.
            </p>
          ) : null}
          <AccessStatus />
          {access?.view.step === "waiting" ? null : (
            <Button
              size="sm"
              variant="primary"
              className="self-start"
              onClick={() => {
                onDone();
                relaunch(item);
              }}
            >
              Relaunch on {name}
            </Button>
          )}
          {quick.access ? (
            <>
              <p className="mt-1 text-xs text-muted-foreground">Or run this on {name}:</p>
              <RemoteCommand command={accessCommand(quick.access)} />
            </>
          ) : null}
        </div>
      );
  }
}

/** The access request open with this machine: waiting with Cancel, or how it ended. */
function AccessStatus() {
  const { access, cancelAsk } = useRemoteControls();
  if (!access) return null;
  const { view } = access;
  if (view.step === "waiting") {
    return (
      <div className="flex items-center gap-2 text-muted-foreground">
        <SpinnerIcon className="size-3.5 shrink-0 animate-spin" />
        <span className="min-w-0 flex-1">{view.text}</span>
        <Button size="xs" variant="ghost" onClick={cancelAsk}>
          Cancel
        </Button>
      </div>
    );
  }
  return (
    <p
      role="status"
      className={cn(view.ok ? "text-emerald-600 dark:text-emerald-400" : "text-muted-foreground")}
    >
      {view.text}
    </p>
  );
}
