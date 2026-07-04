"use client";

import { revalidateLogic } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { z } from "zod";

import type { FolderInitServiceDto, ProjectInfoDto, ProjectServiceInfoDto } from "@/types/roxy";

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
import { bootstrapQueries, projectMutations, utilityMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { normalizeDomain, normalizeSuffix } from "@/lib/validators/validator.create-project";

import { DomainTargetFields } from "./create-dialog/domain-target-fields";

interface ProjectAddServiceDialogProps {
  project: ProjectInfoDto;
  services: ProjectServiceInfoDto[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const addServiceSchema = z.object({
  serviceId: z.string().trim().min(1, "Select an app to add"),
  domain: z.string().trim().min(1, "Domain is required"),
  target: z.string().trim().min(1, "Target is required"),
});

type AddServiceInput = z.infer<typeof addServiceSchema>;

const defaultValues: AddServiceInput = { serviceId: "", domain: "", target: "" };

export const ProjectAddServiceDialog = ({
  project,
  services,
  open,
  onOpenChange,
}: ProjectAddServiceDialogProps) => {
  const queryClient = useQueryClient();
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [candidates, setCandidates] = useState<FolderInitServiceDto[]>([]);

  const detectInitPlan = useMutation(projectMutations.detectInitPlan());
  const addService = useMutation(projectMutations.addService(queryClient));
  const availablePort = useMutation(utilityMutations.availablePort());

  const { data } = useQuery(bootstrapQueries.read());
  const normalizedSuffix = normalizeSuffix(data?.environment.fixedDomainSuffix ?? "");

  const form = useAppForm({
    defaultValues,
    validationLogic: revalidateLogic({
      mode: "submit",
      modeAfterSubmission: "change",
    }),
    validators: { onDynamic: addServiceSchema },
    onSubmit: async ({ value }: { value: AddServiceInput }) => {
      setSubmitError(null);
      const candidate = candidates.find((service) => service.id === value.serviceId);
      if (!candidate) {
        setSubmitError("Select an app to add.");
        return;
      }

      try {
        await addService.mutateAsync({
          projectId: project.id,
          domain: normalizeDomain(value.domain, normalizedSuffix),
          target: value.target,
          serviceName: candidate.serviceName,
          servicePath: candidate.servicePath,
          command: candidate.command,
        });
        captureDesktopEvent("desktop_service_added", {
          detected_command: candidate.command !== null,
        });
        form.reset();
        onOpenChange(false);
      } catch (rawError) {
        setSubmitError(rawError instanceof Error ? rawError.message : String(rawError));
      }
    },
  });

  const selectCandidate = (candidate: FolderInitServiceDto) => {
    form.setFieldValue("serviceId", candidate.id);
    form.setFieldValue("domain", stripSuffix(candidate.domain, normalizedSuffix));
    form.setFieldValue("target", candidate.target);
  };

  useEffect(() => {
    if (!open || !project.rootPath) return;

    let cancelled = false;
    setCandidates([]);
    detectInitPlan
      .mutateAsync({ rootPath: project.rootPath })
      .then((plan) => {
        if (cancelled) return;
        const remaining = plan.services.filter(
          (candidate) => !isAlreadyConfigured(candidate, services),
        );
        setCandidates(remaining);
        if (remaining.length > 0) {
          form.setFieldValue("serviceId", remaining[0].id);
          form.setFieldValue("domain", stripSuffix(remaining[0].domain, normalizedSuffix));
          form.setFieldValue("target", remaining[0].target);
        }
      })
      .catch((rawError: unknown) => {
        if (cancelled) return;
        setSubmitError(rawError instanceof Error ? rawError.message : String(rawError));
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
        onOpenChange(nextOpen);
        if (nextOpen) return;
        setSubmitError(null);
        form.reset();
      }}
    >
      <DialogContent className="sm:max-w-lg">
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
              <DialogTitle>Add service</DialogTitle>
              <DialogDescription>
                Add a detected app from{" "}
                <span className="font-medium text-foreground">{project.name}</span> as a new service
                with its own domain.
              </DialogDescription>
            </DialogHeader>

            <DialogBody className="space-y-4">
              {candidates.length === 0 ? (
                <div className="rounded-xl border border-dashed border-border px-4 py-6 text-center text-sm text-muted-foreground">
                  {detectInitPlan.isPending
                    ? "Inspecting project folder…"
                    : "Every detected app already has a service. Add apps to the project folder first."}
                </div>
              ) : (
                <form.AppField name="serviceId">
                  {(field) => (
                    <div className="divide-y divide-border overflow-hidden rounded-xl border border-border">
                      {candidates.map((candidate) => {
                        const isSelected = field.state.value === candidate.id;
                        return (
                          <label
                            key={candidate.id}
                            className={cn(
                              "flex cursor-pointer items-center gap-3 bg-background px-3 py-2 transition-colors hover:bg-muted/40",
                              isSelected && "bg-muted/40",
                            )}
                          >
                            <input
                              type="radio"
                              name="add-service-candidate"
                              checked={isSelected}
                              onChange={() => selectCandidate(candidate)}
                              onBlur={field.handleBlur}
                              aria-label={`Add ${candidate.serviceName}`}
                              className="size-4 shrink-0 accent-foreground"
                            />
                            <div className="flex min-w-0 flex-1 items-baseline gap-2">
                              <span className="truncate text-sm font-medium text-foreground">
                                {candidate.serviceName}
                              </span>
                              <span className="truncate font-mono text-xs text-muted-foreground">
                                {candidate.servicePath ?? candidate.displayPath}
                              </span>
                            </div>
                            {candidate.command ? (
                              <code className="hidden shrink-0 truncate rounded-md bg-muted/60 px-1.5 py-0.5 font-mono text-xs text-muted-foreground sm:inline">
                                {candidate.command.join(" ")}
                              </code>
                            ) : null}
                          </label>
                        );
                      })}
                    </div>
                  )}
                </form.AppField>
              )}

              {candidates.length > 0 && (
                <DomainTargetFields
                  form={form}
                  fields={{ domain: "domain", target: "target" }}
                  domainSuffix={normalizedSuffix}
                  onFillPort={() => void fillPort()}
                  isFindingPort={availablePort.isPending}
                />
              )}

              {submitError && (
                <p role="alert" className="text-xs text-destructive">
                  {submitError}
                </p>
              )}
            </DialogBody>
            <DialogFooter>
              <DialogClose render={<Button variant="outline">Cancel</Button>} />
              <form.SubmitButton pendingLabel="Adding…" disabled={candidates.length === 0}>
                Add service
              </form.SubmitButton>
            </DialogFooter>
          </form>
        </form.AppForm>
      </DialogContent>
    </Dialog>
  );
};

function stripSuffix(value: string, suffix: string): string {
  return suffix && value.endsWith(suffix) ? value.slice(0, -suffix.length) : value;
}

function isAlreadyConfigured(
  candidate: FolderInitServiceDto,
  services: ProjectServiceInfoDto[],
): boolean {
  const candidatePath =
    candidate.servicePath === null ? null : normalizePath(candidate.servicePath);
  const candidateName = candidate.serviceName.toLowerCase();

  return services.some((service) => {
    if (candidatePath !== null && service.path !== null) {
      return normalizePath(service.path) === candidatePath;
    }
    return (
      service.id.toLowerCase() === candidateName || service.name.toLowerCase() === candidateName
    );
  });
}

function normalizePath(path: string): string {
  return path.replaceAll("\\", "/").replace(/^\/+|\/+$/g, "");
}
