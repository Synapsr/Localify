"use client";

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { type PropsWithChildren, useState } from "react";

import type { DomainInfoDto } from "@/types/localify";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { domainMutations } from "@/lib/queries";

interface ProjectDeleteDomainDialogProps extends PropsWithChildren {
  domain: DomainInfoDto;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

export const ProjectDeleteDomainDialog = ({
  domain,
  children,
  open: controlledOpen,
  onOpenChange,
}: ProjectDeleteDomainDialogProps) => {
  const queryClient = useQueryClient();
  const [uncontrolledOpen, setUncontrolledOpen] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  const unregister = useMutation(domainMutations.unregister(queryClient));

  const { pattern, domain: domainName, wildcard, routes } = domain.registration;
  const open = controlledOpen ?? uncontrolledOpen;

  const setOpen = (nextOpen: boolean) => {
    if (controlledOpen === undefined) {
      setUncontrolledOpen(nextOpen);
    }
    onOpenChange?.(nextOpen);
  };

  const handleDelete = async () => {
    setSubmitError(null);
    try {
      await unregister.mutateAsync({ domain: domainName, wildcard });
      setOpen(false);
      setSubmitError(null);
    } catch (rawError) {
      setSubmitError(rawError instanceof Error ? rawError.message : String(rawError));
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => {
        setOpen(nextOpen);
        if (nextOpen) return;
        setSubmitError(null);
      }}
    >
      {children}
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Remove domain?</DialogTitle>
          <DialogDescription>
            This unregisters <span className="font-mono text-foreground">{pattern}</span> and
            removes its {routes.length} route{routes.length === 1 ? "" : "s"}.
          </DialogDescription>
        </DialogHeader>

        {submitError && (
          <p role="alert" className="text-xs text-[--color-danger]">
            {submitError}
          </p>
        )}

        <DialogFooter>
          <DialogClose render={<Button variant="outline" />} disabled={unregister.isPending}>
            Cancel
          </DialogClose>
          <Button
            type="button"
            variant="danger"
            loading={unregister.isPending}
            onClick={handleDelete}
          >
            Remove domain
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
