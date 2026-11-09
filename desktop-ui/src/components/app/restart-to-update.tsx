"use client";

import { useMutation } from "@tanstack/react-query";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { systemMutations } from "@/lib/queries";
import { restartCloses } from "@/lib/util.app-update";
import { useTerminal } from "@/providers/terminal-provider";

interface RestartToUpdateProps {
  appName: string;
  version: string;
  size?: "xs" | "sm";
}

/**
 * `Restart to update`: names the tabs a restart closes, then hands the install to Localify's
 * installer and quits. The app never restarts by itself.
 */
export const RestartToUpdate = ({ appName, version, size = "sm" }: RestartToUpdateProps) => {
  const { sessions } = useTerminal();
  const restart = useMutation(systemMutations.restartToUpdate());
  const [open, setOpen] = useState(false);
  const closes = restartCloses(sessions);
  const closesNothing = closes.services.length === 0 && closes.shells.length === 0;

  return (
    <>
      <Button
        size={size}
        variant="primary"
        onClick={() => {
          restart.reset();
          setOpen(true);
        }}
      >
        Restart to update
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>Restart to update?</DialogTitle>
            <DialogDescription>
              {appName} quits, installs {version}, then opens again.
            </DialogDescription>
          </DialogHeader>
          <DialogBody className="space-y-3 text-sm">
            {closesNothing ? (
              <p className="text-muted-foreground">No service or shell tab is open.</p>
            ) : (
              <>
                <ClosedTabs label="Services that stop" names={closes.services} />
                <ClosedTabs label="Shells that close" names={closes.shells} />
              </>
            )}
            {restart.error ? (
              <p className="text-xs leading-normal text-destructive">{restart.error.message}</p>
            ) : null}
          </DialogBody>
          <DialogFooter>
            <DialogClose render={<Button variant="outline" />} disabled={restart.isPending}>
              Cancel
            </DialogClose>
            <Button
              type="button"
              variant="primary"
              loading={restart.isPending}
              onClick={() => restart.mutate()}
            >
              Restart to update
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
};

const ClosedTabs = ({ label, names }: { label: string; names: string[] }) => {
  if (names.length === 0) return null;

  return (
    <div className="space-y-1">
      <p className="text-xs font-medium text-muted-foreground">{label}</p>
      <ul className="space-y-0.5">
        {names.map((name, index) => (
          // Two shells in one checkout share a name.
          <li key={`${name}-${index}`} className="truncate text-foreground">
            {name}
          </li>
        ))}
      </ul>
    </div>
  );
};
