"use client";

import type { ReactNode } from "react";

import { Link } from "@tanstack/react-router";
import { toast } from "sonner";

import { SpinnerIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { PAIRING_CANCELLED, SETTINGS_REMOTE_ANCHOR } from "@/lib/util.remote";
import { useRemote } from "@/providers/remote-provider";

/**
 * Both sides of a pairing, in the words of docs/remote-machines.md, "Pairing": the machine
 * that asked, and this Mac when another one asks.
 */
export function RemotePairingDialogs() {
  return (
    <>
      <OutgoingPairingDialog />
      <IncomingPairingDialog />
    </>
  );
}

function PairingCode({ code }: { code: string }) {
  return (
    <span className="mt-3 block w-fit rounded-lg bg-background px-4 py-2 font-mono text-3xl font-semibold tracking-[0.15em] text-foreground ring-1 ring-border/60 select-all">
      {code}
    </span>
  );
}

function Waiting({ children }: { children: ReactNode }) {
  return (
    <span className="inline-flex items-center gap-2 text-xs text-muted-foreground">
      <SpinnerIcon className="size-3.5 animate-spin" />
      {children}
    </span>
  );
}

function failed(title: string, error: unknown) {
  toast.error(title, { description: error instanceof Error ? error.message : String(error) });
}

function OutgoingPairingDialog() {
  const { outgoing, cancelOutgoing, confirmOutgoing, dismissOutgoing } = useRemote();
  if (!outgoing) return null;
  const name = outgoing.machine.name;
  const close =
    outgoing.step === "asking" || outgoing.step === "code" ? cancelOutgoing : dismissOutgoing;

  return (
    <Dialog open onOpenChange={(next) => (next ? undefined : close())}>
      <DialogContent className="sm:max-w-md">
        {outgoing.step === "asking" ? (
          <>
            <DialogHeader>
              <DialogTitle>Pair with {name}</DialogTitle>
              <DialogDescription>Asking {name} to pair…</DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="outline" onClick={cancelOutgoing}>
                Cancel
              </Button>
            </DialogFooter>
          </>
        ) : null}

        {outgoing.step === "code" ? (
          <>
            <DialogHeader>
              <DialogTitle>Pair with {name}</DialogTitle>
              <DialogDescription render={<div />}>
                Check that {name} shows this code:
                <PairingCode code={outgoing.code} />
              </DialogDescription>
            </DialogHeader>
            <DialogFooter className="items-center">
              {outgoing.answered ? <Waiting>Waiting for {name} to confirm…</Waiting> : null}
              <Button variant="outline" onClick={cancelOutgoing}>
                Cancel
              </Button>
              {outgoing.answered ? null : (
                <Button
                  variant="primary"
                  onClick={() =>
                    void confirmOutgoing().catch((error: unknown) =>
                      failed("Could not confirm the code", error),
                    )
                  }
                >
                  Codes match
                </Button>
              )}
            </DialogFooter>
          </>
        ) : null}

        {outgoing.step === "paired" ? (
          <>
            <DialogHeader>
              <DialogTitle>Paired with {name}</DialogTitle>
              <DialogDescription>
                {name} shows as “Paired · not sharing” until it turns on sharing in Settings →
                Remote access on {name}.
              </DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="primary" onClick={dismissOutgoing}>
                Done
              </Button>
            </DialogFooter>
          </>
        ) : null}

        {outgoing.step === "cancelled" ? (
          <>
            <DialogHeader>
              <DialogTitle>Pair with {name}</DialogTitle>
              <DialogDescription>{outgoing.message || PAIRING_CANCELLED}</DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="outline" onClick={dismissOutgoing}>
                Close
              </Button>
            </DialogFooter>
          </>
        ) : null}

        {outgoing.step === "partial" ? (
          <>
            <DialogHeader>
              <DialogTitle>Pairing with {name} did not finish</DialogTitle>
              <DialogDescription>{outgoing.message}</DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="outline" onClick={dismissOutgoing}>
                Close
              </Button>
            </DialogFooter>
          </>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function IncomingPairingDialog() {
  const { incoming, answerIncoming, dismissIncoming } = useRemote();
  if (!incoming) return null;
  const answer = (accept: boolean) =>
    void answerIncoming(accept).catch((error: unknown) =>
      failed("Could not answer the pairing", error),
    );
  const close = incoming.step === "code" ? () => answer(false) : dismissIncoming;

  return (
    <Dialog open onOpenChange={(next) => (next ? undefined : close())}>
      <DialogContent className="sm:max-w-md">
        {incoming.step === "code" ? (
          <>
            <DialogHeader>
              <DialogTitle>{incoming.name} wants to pair with this Mac</DialogTitle>
              <DialogDescription render={<div />}>
                Pair only if {incoming.name} shows the same code:
                <PairingCode code={incoming.code} />
                <span className="mt-3 block">
                  {incoming.name} will then see this Mac's name. It sees projects and output, or
                  controls services, only after you turn that on in Settings → Remote access.
                </span>
              </DialogDescription>
            </DialogHeader>
            <DialogFooter className="items-center">
              {incoming.answered ? <Waiting>Waiting for {incoming.name}…</Waiting> : null}
              <Button variant="outline" onClick={() => answer(false)}>
                Cancel
              </Button>
              {incoming.answered ? null : (
                <Button variant="primary" onClick={() => answer(true)}>
                  Codes match, pair
                </Button>
              )}
            </DialogFooter>
          </>
        ) : null}

        {incoming.step === "paired" ? (
          <>
            <DialogHeader>
              <DialogTitle>Paired with {incoming.name}</DialogTitle>
              <DialogDescription>
                {incoming.name} sees this Mac's name only. Choose what it may see in{" "}
                <Link
                  to="/settings"
                  hash={SETTINGS_REMOTE_ANCHOR}
                  onClick={dismissIncoming}
                  className="font-medium text-foreground underline underline-offset-3"
                >
                  Settings → Remote access
                </Link>
                .
              </DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="primary" onClick={dismissIncoming}>
                Done
              </Button>
            </DialogFooter>
          </>
        ) : null}

        {incoming.step === "cancelled" ? (
          <>
            <DialogHeader>
              <DialogTitle>Pairing with {incoming.name}</DialogTitle>
              <DialogDescription>{PAIRING_CANCELLED}</DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="outline" onClick={dismissIncoming}>
                Close
              </Button>
            </DialogFooter>
          </>
        ) : null}

        {incoming.step === "partial" ? (
          <>
            <DialogHeader>
              <DialogTitle>Pairing with {incoming.name} did not finish</DialogTitle>
              <DialogDescription>{incoming.message}</DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="outline" onClick={dismissIncoming}>
                Close
              </Button>
            </DialogFooter>
          </>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}
