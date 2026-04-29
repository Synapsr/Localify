"use client";

import { revalidateLogic } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type PropsWithChildren, useEffect, useState } from "react";
import { z } from "zod";

import type { ProjectInfoDto } from "@/types/roxy";

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
import { captureDesktopEvent } from "@/lib/analytics";
import { bootstrapQueries, domainMutations, utilityMutations } from "@/lib/queries";
import {
  type ProjectDomainEntry,
  emptyDomainEntry,
  normalizeDomain,
  normalizeSuffix,
  projectDomainEntrySchema,
} from "@/lib/validators/validator.create-project";

import { DomainTargetFields } from "./create-dialog/domain-target-fields";

interface ProjectAddDomainDialogProps extends PropsWithChildren {
  project: ProjectInfoDto;
}

const addDomainSchema = projectDomainEntrySchema.superRefine((value, ctx) => {
  if (value.domain.length === 0) {
    ctx.addIssue({ code: "custom", path: ["domain"], message: "Domain is required" });
  }
  if (value.target.length === 0) {
    ctx.addIssue({ code: "custom", path: ["target"], message: "Target is required" });
  }
});

type AddDomainInput = z.infer<typeof addDomainSchema>;

const defaultValues: AddDomainInput = emptyDomainEntry();

export const ProjectAddDomainDialog = ({ project, children }: ProjectAddDomainDialogProps) => {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);

  const registerDomain = useMutation(domainMutations.register(queryClient));
  const availablePort = useMutation(utilityMutations.availablePort());

  const { data } = useQuery(bootstrapQueries.read());
  const suffix = data?.environment.fixedDomainSuffix ?? "";
  const normalizedSuffix = normalizeSuffix(suffix);

  const form = useAppForm({
    defaultValues,
    validationLogic: revalidateLogic({
      mode: "submit",
      modeAfterSubmission: "change",
    }),
    validators: { onDynamic: addDomainSchema },
    onSubmit: async ({ value }: { value: ProjectDomainEntry }) => {
      setSubmitError(null);
      try {
        await registerDomain.mutateAsync({
          domain: normalizeDomain(value.domain, normalizedSuffix),
          wildcard: value.wildcard,
          routes: [value.target],
          projectId: project.id,
        });
        captureDesktopEvent("desktop_domain_added", {
          wildcard: value.wildcard,
          project_attached: true,
        });
        form.reset();
        setOpen(false);
      } catch (rawError) {
        setSubmitError(rawError instanceof Error ? rawError.message : String(rawError));
      }
    },
  });

  useEffect(() => {
    if (!open) return;
    if ((form.getFieldValue("target") ?? "").trim().length > 0) return;

    let cancelled = false;
    availablePort
      .mutateAsync()
      .then(({ port }) => {
        if (cancelled) return;
        const stillEmpty = (form.getFieldValue("target") ?? "").trim().length === 0;
        if (stillEmpty) {
          form.setFieldValue("target", String(port));
        }
      })
      .catch(() => {
        // User can still press "Free port" manually.
      });

    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

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
        form.reset();
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
              <DialogTitle>Add domain</DialogTitle>
              <DialogDescription>
                Attach a new local domain to{" "}
                <span className="font-medium text-foreground">{project.name}</span>.
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
              <form.SubmitButton pendingLabel="Adding…">Add domain</form.SubmitButton>
            </DialogFooter>
          </form>
        </form.AppForm>
      </DialogContent>
    </Dialog>
  );
};
