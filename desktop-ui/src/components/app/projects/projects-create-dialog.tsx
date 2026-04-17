"use client";
import { revalidateLogic } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { PropsWithChildren, useEffect, useRef, useState } from "react";

import type { CreateProjectDomainRequest } from "@/types/roxy";

import { XIcon } from "@/components/icons";
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
import { useAppForm } from "@/hooks/form/form";
import { bootstrapQueries, projectMutations, utilityMutations } from "@/lib/queries";
import {
  CreateProjectInput,
  createProjectSchema,
  emptyDomainEntry,
  normalizeDomain,
  normalizeSuffix,
  slugifyProjectName,
} from "@/lib/validators/validator.create-project";

import {
  consumeQueuedProjectCreateDialogOpen,
  subscribeToProjectCreateDialogOpenRequest,
} from "./util.project-create-dialog";

const defaultValues: CreateProjectInput = {
  name: "",
  domains: [emptyDomainEntry()],
};

export const ProjectsCreateDialog = ({ children }: PropsWithChildren) => {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);

  const createProject = useMutation(projectMutations.create(queryClient));
  const availablePort = useMutation(utilityMutations.availablePort());

  const { data } = useQuery(bootstrapQueries.read());
  const suffix = data?.environment.fixedDomainSuffix ?? "";
  const normalizedSuffix = normalizeSuffix(suffix);

  // Tracks the auto-derived slug so we can detect when the user customises the
  // primary domain and stop syncing it from the project name.
  const lastAutoSlugRef = useRef("");

  const [submitError, setSubmitError] = useState<string | null>(null);

  useEffect(() => {
    const openDialog = () => {
      setOpen(true);
    };

    if (consumeQueuedProjectCreateDialogOpen()) {
      openDialog();
    }

    return subscribeToProjectCreateDialogOpenRequest(openDialog);
  }, []);

  const form = useAppForm({
    defaultValues,
    validationLogic: revalidateLogic({
      mode: "submit",
      modeAfterSubmission: "change",
    }),
    validators: { onDynamic: createProjectSchema },
    onSubmit: async ({ value }) => {
      setSubmitError(null);

      try {
        const domains: CreateProjectDomainRequest[] = value.domains
          .filter((entry, index) => {
            if (index === 0) {
              return entry.domain.length > 0 || entry.target.length > 0;
            }
            return true;
          })
          .map((entry) => ({
            domain: normalizeDomain(entry.domain, normalizedSuffix),
            wildcard: entry.wildcard,
            routes: [entry.target],
          }));

        await Promise.all([
          createProject.mutateAsync({
            name: value.name,
            domains,
          }),
          new Promise((resolve) => setTimeout(resolve, 1000)),
        ]);

        form.reset();
        lastAutoSlugRef.current = "";
        setOpen(false);
      } catch (rawError) {
        const message = rawError instanceof Error ? rawError.message : String(rawError);
        setSubmitError(message);
      }
    },
  });

  // Auto-fill the primary target with a free port the first time the dialog opens.
  useEffect(() => {
    if (!open) return;
    const current = form.getFieldValue("domains")?.[0]?.target ?? "";
    if (current.trim().length > 0) return;

    let cancelled = false;
    availablePort
      .mutateAsync()
      .then(({ port }) => {
        if (cancelled) return;
        const stillEmpty =
          (form.getFieldValue("domains")?.[0]?.target ?? "").trim().length === 0;
        if (stillEmpty) {
          form.setFieldValue("domains[0].target", String(port));
        }
      })
      .catch(() => {
        // Ignore — user can still press "Free port" manually.
      });

    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const fillPort = async (index: number) => {
    setSubmitError(null);
    try {
      const { port } = await availablePort.mutateAsync();
      form.setFieldValue(`domains[${index}].target`, String(port));
    } catch (rawError) {
      const message = rawError instanceof Error ? rawError.message : String(rawError);
      setSubmitError(message);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => {
        setOpen(nextOpen);
        if (nextOpen) {
          return;
        }
        setSubmitError(null);
        lastAutoSlugRef.current = "";
        form.reset();
      }}
    >
      {children}
      <DialogContent className="sm:max-w-lg">
        <form.AppForm>
          <form
            noValidate
            className="space-y-6"
            onSubmit={(event) => {
              event.preventDefault();
              void form.handleSubmit();
            }}
          >
            <DialogHeader>
              <DialogTitle>New project</DialogTitle>
              <DialogDescription>
                Create an empty project, or set up your first local domains immediately.
              </DialogDescription>
            </DialogHeader>

            <form.AppField
              name="name"
              listeners={{
                onChange: ({ value }) => {
                  const nextSlug = slugifyProjectName(value);
                  const currentDomain = form.getFieldValue("domains")?.[0]?.domain ?? "";
                  if (currentDomain === "" || currentDomain === lastAutoSlugRef.current) {
                    form.setFieldValue("domains[0].domain", nextSlug);
                    lastAutoSlugRef.current = nextSlug;
                  }
                },
              }}
            >
              {(field) => <field.TextField label="Project name" placeholder="Test 1" autoFocus />}
            </form.AppField>

            <form.Field name="domains" mode="array">
              {(domainsField) => (
                <div className="space-y-3">
                  <div className="flex items-center justify-between">
                    <h3 className="text-sm font-medium text-[--color-ink]">Domains</h3>
                    <span className="text-xs text-[--color-muted]">
                      {domainsField.state.value.length} configured
                    </span>
                  </div>

                  <div className="space-y-2">
                    {domainsField.state.value.map((_, index) => (
                      <div
                        key={index}
                        className="space-y-3 rounded-xl border border-[--color-border] p-4"
                      >
                        <div className="flex items-center justify-between">
                          <span className="text-xs font-medium text-[--color-muted]">
                            {index === 0 ? "Primary" : `Domain ${index + 1}`}
                          </span>
                          {index > 0 && (
                            <Button
                              type="button"
                              variant="ghost"
                              size="icon-xs"
                              onClick={() => domainsField.removeValue(index)}
                              aria-label="Remove domain"
                            >
                              <XIcon />
                            </Button>
                          )}
                        </div>

                        <div className="grid gap-4 sm:grid-cols-2">
                          <form.AppField name={`domains[${index}].domain`}>
                            {(field) => (
                              <field.TextField
                                label="Domain"
                                placeholder="my-app"
                                inputSuffix={normalizedSuffix}
                              />
                            )}
                          </form.AppField>

                          <form.AppField name={`domains[${index}].target`}>
                            {(field) => (
                              <field.TextField
                                label="Target"
                                placeholder="3000"
                                rightAction={
                                  <button
                                    type="button"
                                    onClick={() => void fillPort(index)}
                                    disabled={availablePort.isPending}
                                    className="text-xs text-[--color-muted] hover:text-[--color-ink] disabled:cursor-not-allowed disabled:opacity-50"
                                  >
                                    {availablePort.isPending ? "Finding…" : "Free port"}
                                  </button>
                                }
                              />
                            )}
                          </form.AppField>
                        </div>

                        <form.AppField name={`domains[${index}].wildcard`}>
                          {(field) => (
                            <field.CheckboxField label="Match all subdomains (wildcard)" />
                          )}
                        </form.AppField>
                      </div>
                    ))}
                  </div>

                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => domainsField.pushValue(emptyDomainEntry())}
                  >
                    + Add domain
                  </Button>
                </div>
              )}
            </form.Field>

            {submitError && (
              <p role="alert" className="text-xs text-[--color-danger]">
                {submitError}
              </p>
            )}

            <DialogFooter>
              <DialogClose render={<Button variant="outline">Cancel</Button>} />
              <form.SubmitButton pendingLabel="Creating…">Create project</form.SubmitButton>
            </DialogFooter>
          </form>
        </form.AppForm>
      </DialogContent>
    </Dialog>
  );
};
