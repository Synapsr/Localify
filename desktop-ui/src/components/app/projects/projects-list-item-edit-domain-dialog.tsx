"use client";

import { revalidateLogic } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type PropsWithChildren, useEffect, useState } from "react";
import { z } from "zod";

import type { DomainInfoDto } from "@/types/roxy";

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
import { useAppForm } from "@/hooks/form/form";
import { bootstrapQueries, domainMutations, utilityMutations } from "@/lib/queries";
import {
  type ProjectDomainEntry,
  emptyDomainEntry,
  normalizeDomain,
  normalizeSuffix,
  projectDomainEntrySchema,
} from "@/lib/validators/validator.create-project";

import { DomainTargetFields } from "./create-dialog/domain-target-fields";

interface ProjectEditDomainDialogProps extends PropsWithChildren {
  domain: DomainInfoDto;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

const editDomainSchema = projectDomainEntrySchema.superRefine((value, ctx) => {
  if (value.domain.length === 0) {
    ctx.addIssue({ code: "custom", path: ["domain"], message: "Domain is required" });
  }
  if (value.target.length === 0) {
    ctx.addIssue({ code: "custom", path: ["target"], message: "Target is required" });
  }
});

type EditDomainInput = z.infer<typeof editDomainSchema>;

function stripSuffix(value: string, suffix: string): string {
  if (suffix.length === 0) return value;
  return value.endsWith(suffix) ? value.slice(0, -suffix.length) : value;
}

function buildDefaults(domain: DomainInfoDto, normalizedSuffix: string): EditDomainInput {
  return {
    domain: stripSuffix(domain.registration.domain, normalizedSuffix),
    target: domain.registration.routes[0]?.target ?? "",
    wildcard: domain.registration.wildcard,
  };
}

export const ProjectEditDomainDialog = ({
  domain,
  children,
  open: controlledOpen,
  onOpenChange,
}: ProjectEditDomainDialogProps) => {
  const queryClient = useQueryClient();
  const [uncontrolledOpen, setUncontrolledOpen] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  const updateDomain = useMutation(domainMutations.update(queryClient));
  const availablePort = useMutation(utilityMutations.availablePort());

  const { data } = useQuery(bootstrapQueries.read());
  const suffix = data?.environment.fixedDomainSuffix ?? "";
  const normalizedSuffix = normalizeSuffix(suffix);
  const open = controlledOpen ?? uncontrolledOpen;

  const setOpen = (nextOpen: boolean) => {
    if (controlledOpen === undefined) {
      setUncontrolledOpen(nextOpen);
    }
    onOpenChange?.(nextOpen);
  };

  const form = useAppForm({
    defaultValues: emptyDomainEntry() as EditDomainInput,
    validationLogic: revalidateLogic({
      mode: "submit",
      modeAfterSubmission: "change",
    }),
    validators: { onDynamic: editDomainSchema },
    onSubmit: async ({ value }: { value: ProjectDomainEntry }) => {
      setSubmitError(null);
      const previousDomain = domain.registration.domain;
      const previousWildcard = domain.registration.wildcard;
      try {
        await updateDomain.mutateAsync({
          currentDomain: previousDomain,
          currentWildcard: previousWildcard,
          domain: normalizeDomain(value.domain, normalizedSuffix),
          wildcard: value.wildcard,
          routes: [value.target],
          projectId: domain.projectId,
        });
        setOpen(false);
        setSubmitError(null);
      } catch (rawError) {
        setSubmitError(rawError instanceof Error ? rawError.message : String(rawError));
      }
    },
  });

  useEffect(() => {
    if (!open) return;
    form.reset(buildDefaults(domain, normalizedSuffix));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, normalizedSuffix, domain.registration.domain, domain.registration.wildcard]);

  const fillPort = async () => {
    setSubmitError(null);
    try {
      const { port } = await availablePort.mutateAsync();
      form.setFieldValue("target", String(port));
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
        form.reset(buildDefaults(domain, normalizedSuffix));
      }}
    >
      {children}
      <DialogContent className="sm:max-w-md">
        <form.AppForm>
          <form
            noValidate
            className="space-y-5"
            onSubmit={(event) => {
              event.preventDefault();
              void form.handleSubmit();
            }}
          >
            <DialogHeader>
              <DialogTitle>Edit domain</DialogTitle>
              <DialogDescription>
                Update the pattern or target for{" "}
                <span className="font-mono text-foreground">{domain.registration.pattern}</span>.
              </DialogDescription>
            </DialogHeader>

            <DialogBody className="space-y-4">
              <DomainTargetFields
                form={form}
                fields={{ domain: "domain", target: "target" }}
                domainSuffix={normalizedSuffix}
                onFillPort={() => void fillPort()}
                isFindingPort={availablePort.isPending}
              />

              <form.AppField name="wildcard">
                {(field) => <field.CheckboxField label="Match all subdomains (wildcard)" />}
              </form.AppField>

              {submitError && (
                <p role="alert" className="text-xs text-[--color-danger]">
                  {submitError}
                </p>
              )}
            </DialogBody>
            <DialogFooter>
              <DialogClose render={<Button variant="outline">Cancel</Button>} />
              <form.SubmitButton pendingLabel="Saving…">Save changes</form.SubmitButton>
            </DialogFooter>
          </form>
        </form.AppForm>
      </DialogContent>
    </Dialog>
  );
};
