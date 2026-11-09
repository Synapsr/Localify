"use client";

import { useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { accessRequestTitle } from "@/lib/util.remote-controls";
import { useRemote } from "@/providers/remote-provider";

/**
 * Another machine asks this Mac for remote controls (docs/remote-machines.md, "Asking for
 * access", step 4). The switches are machine-wide, so the question says so. Only `Allow` or
 * `Don't allow` answers it: Escape and a click outside leave it to expire.
 */
export function RemoteAccessDialog() {
  const { access } = useRemote();
  if (!access) return null;
  // A fresh dialog, and a cleared checkbox, for each request.
  return <AccessRequestDialog key={access.requestId} />;
}

function AccessRequestDialog() {
  const { access, answerAccess } = useRemote();
  const [suppress, setSuppress] = useState(false);
  // Focus lands on the dialog, not a button, so typing in a terminal can't answer it.
  const popup = useRef<HTMLDivElement>(null);
  if (!access) return null;

  return (
    <Dialog open onOpenChange={() => undefined}>
      <DialogContent
        ref={popup}
        tabIndex={-1}
        initialFocus={popup}
        showCloseButton={false}
        className="sm:max-w-md"
      >
        <DialogHeader>
          <DialogTitle>{accessRequestTitle(access.name, access.fingerprint)}</DialogTitle>
          <DialogDescription render={<div />}>
            <p>
              Allow every paired machine to start, stop and restart services on this Mac, until you
              turn it off in Settings?
            </p>
            {access.wants === "backgroundStarts" ? (
              <p className="mt-2">…also when Localify is closed here, in the background.</p>
            ) : null}
          </DialogDescription>
          <label className="mt-2 flex items-center gap-2 text-sm text-foreground">
            <input
              type="checkbox"
              checked={suppress}
              disabled={access.answering}
              onChange={(event) => setSuppress(event.target.checked)}
              className="size-4 accent-foreground"
            />
            Don't ask again from {access.name}
          </label>
          {access.error ? (
            <p role="alert" className="mt-2 text-sm text-destructive">
              Could not answer the request: {access.error}
            </p>
          ) : null}
        </DialogHeader>
        <DialogFooter>
          <Button
            variant="outline"
            disabled={access.answering}
            onClick={() => answerAccess(false, suppress)}
          >
            Don't allow
          </Button>
          <Button
            variant="primary"
            disabled={access.answering}
            onClick={() => answerAccess(true, suppress)}
          >
            Allow
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
