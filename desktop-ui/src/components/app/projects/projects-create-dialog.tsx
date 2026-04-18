"use client";

import { open as openDirectoryDialog } from "@tauri-apps/plugin-dialog";
import { revalidateLogic } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type PropsWithChildren, useEffect, useRef, useState } from "react";

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
  type CreateProjectInput,
  createProjectSchema,
  emptyDomainEntry,
  normalizeDomain,
  normalizeSuffix,
  slugifyProjectName,
} from "@/lib/validators/validator.create-project";
import {
  type InitProjectFromFolderInput,
  initProjectFromFolderSchema,
} from "@/lib/validators/validator.init-project-from-folder";

import {
  consumeQueuedProjectCreateDialogOpen,
  subscribeToProjectCreateDialogOpenRequest,
} from "./util.project-create-dialog";

const defaultValues: CreateProjectInput = {
  name: "",
  domains: [emptyDomainEntry()],
};

const defaultFolderValues: InitProjectFromFolderInput = {
  rootPath: "",
  name: "",
  services: [],
};

export const ProjectsCreateDialog = ({ children }: PropsWithChildren) => {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [creationMode, setCreationMode] = useState<"manual" | "folder">("manual");
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [selectedFolderPath, setSelectedFolderPath] = useState("");

  const createProject = useMutation(projectMutations.create(queryClient));
  const detectInitPlan = useMutation(projectMutations.detectInitPlan());
  const initFromFolder = useMutation(projectMutations.initFromFolder(queryClient));
  const availablePort = useMutation(utilityMutations.availablePort());

  const { data } = useQuery(bootstrapQueries.read());
  const suffix = data?.environment.fixedDomainSuffix ?? "";
  const normalizedSuffix = normalizeSuffix(suffix);

  // Tracks the auto-derived slug so we can detect when the user customises the
  // primary domain and stop syncing it from the project name.
  const lastAutoSlugRef = useRef("");

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

        resetDialogState();
      } catch (rawError) {
        const message = rawError instanceof Error ? rawError.message : String(rawError);
        setSubmitError(message);
      }
    },
  });

  const folderForm = useAppForm({
    defaultValues: defaultFolderValues,
    validationLogic: revalidateLogic({
      mode: "submit",
      modeAfterSubmission: "change",
    }),
    validators: { onDynamic: initProjectFromFolderSchema },
    onSubmit: async ({ value }) => {
      setSubmitError(null);

      try {
        await initFromFolder.mutateAsync({
          rootPath: value.rootPath,
          name: value.name,
          services: value.services.map((service) => ({
            id: service.id,
            domain: service.domain,
            target: service.target,
            enabled: service.enabled,
            updateDevScript: service.updateDevScript,
          })),
        });

        resetDialogState();
      } catch (rawError) {
        const message = rawError instanceof Error ? rawError.message : String(rawError);
        setSubmitError(message);
      }
    },
  });

  useEffect(() => {
    if (!open) {
      return;
    }

    const current = form.getFieldValue("domains")?.[0]?.target ?? "";
    if (current.trim().length > 0) {
      return;
    }

    let cancelled = false;
    availablePort
      .mutateAsync()
      .then(({ port }) => {
        if (cancelled) {
          return;
        }

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

  const resetDialogState = () => {
    setSubmitError(null);
    setSelectedFolderPath("");
    setCreationMode("manual");
    lastAutoSlugRef.current = "";
    form.reset();
    folderForm.reset();
    setOpen(false);
  };

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

  const fillFolderPort = async (index: number) => {
    setSubmitError(null);

    try {
      const { port } = await availablePort.mutateAsync();
      folderForm.setFieldValue(`services[${index}].target`, String(port));
    } catch (rawError) {
      const message = rawError instanceof Error ? rawError.message : String(rawError);
      setSubmitError(message);
    }
  };

  const selectFolder = async () => {
    setSubmitError(null);

    try {
      const selected = await openDirectoryDialog({
        directory: true,
        multiple: false,
        title: "Select project folder",
      });

      if (typeof selected !== "string") {
        return;
      }

      setSelectedFolderPath(selected);
      folderForm.setFieldValue("rootPath", selected);
      folderForm.setFieldValue("name", "");
      folderForm.setFieldValue("services", []);

      const plan = await detectInitPlan.mutateAsync({ rootPath: selected });
      folderForm.setFieldValue("rootPath", plan.rootPath);
      folderForm.setFieldValue("name", plan.name);
      folderForm.setFieldValue("services", plan.services);
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
        setSelectedFolderPath("");
        setCreationMode("manual");
        lastAutoSlugRef.current = "";
        form.reset();
        folderForm.reset();
      }}
    >
      {children}
      <DialogContent className="max-h-[calc(100vh-2rem)] overflow-y-auto overscroll-contain sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>New project</DialogTitle>
          <DialogDescription>
            {creationMode === "manual"
              ? "Create an empty project, or set up your first local domains immediately."
              : "Select a local folder, review the detected apps, and initialize it with a flow similar to localify init-project."}
          </DialogDescription>
        </DialogHeader>

        <div className="grid grid-cols-2 gap-2 rounded-xl border border-[--color-border] bg-[--color-surface] p-1">
          <Button
            type="button"
            variant={creationMode === "manual" ? "secondary" : "ghost"}
            onClick={() => {
              setCreationMode("manual");
              setSubmitError(null);
            }}
          >
            Empty project
          </Button>
          <Button
            type="button"
            variant={creationMode === "folder" ? "secondary" : "ghost"}
            onClick={() => {
              setCreationMode("folder");
              setSubmitError(null);
            }}
          >
            From folder
          </Button>
        </div>

        {creationMode === "manual" ? (
          <form.AppForm>
            <form
              noValidate
              className="space-y-6"
              onSubmit={(event) => {
                event.preventDefault();
                void form.handleSubmit();
              }}
            >
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
                {(field) => (
                  <field.TextField label="Project name" placeholder="Test 1" autoFocus />
                )}
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
        ) : (
          <folderForm.AppForm>
            <form
              noValidate
              className="space-y-6"
              onSubmit={(event) => {
                event.preventDefault();
                void folderForm.handleSubmit();
              }}
            >
              <div className="space-y-3 rounded-xl border border-[--color-border] p-4">
                <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                  <div className="space-y-1">
                    <h3 className="text-sm font-medium text-[--color-ink]">Project folder</h3>
                    <p className="text-sm text-[--color-muted]">
                      {selectedFolderPath || "Choose the local folder you want Localify to initialize."}
                    </p>
                  </div>

                  <Button
                    type="button"
                    variant="outline"
                    loading={detectInitPlan.isPending}
                    onClick={() => void selectFolder()}
                  >
                    {selectedFolderPath ? "Change folder" : "Select folder"}
                  </Button>
                </div>
              </div>

              <form.AppField name="name">
                {(field) => (
                  <field.TextField
                    label="Project name"
                    placeholder="my-app"
                    description={
                      selectedFolderPath.length > 0
                        ? "You can rename the Localify project before creating it."
                        : "Pick a folder first to load the suggested project name."
                    }
                  />
                )}
              </form.AppField>

              <folderForm.Field name="services" mode="array">
                {(servicesField) => {
                  const enabledCount = servicesField.state.value.filter(
                    (service) => service.enabled,
                  ).length;

                  return (
                    <div className="space-y-3">
                      <div className="flex items-center justify-between">
                        <div>
                          <h3 className="text-sm font-medium text-[--color-ink]">Detected apps</h3>
                          <p className="text-xs text-[--color-muted]">
                            {enabledCount} selected of {servicesField.state.value.length}
                          </p>
                        </div>
                      </div>

                      {servicesField.state.value.length === 0 ? (
                        <div className="rounded-xl border border-dashed border-[--color-border] px-4 py-6 text-sm text-[--color-muted]">
                          Select a folder to inspect its local apps and suggested domains.
                        </div>
                      ) : (
                        <div className="space-y-3">
                          {servicesField.state.value.map((service, index) => (
                            <div
                              key={service.id}
                              className="space-y-4 rounded-xl border border-[--color-border] p-4"
                            >
                              <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                                <div className="space-y-1">
                                  <p className="text-sm font-medium text-[--color-ink]">
                                    {service.label}
                                  </p>
                                  <p className="text-xs text-[--color-muted]">
                                    {service.displayPath}
                                  </p>
                                </div>

                                <folderForm.AppField name={`services[${index}].enabled`}>
                                  {(field) => (
                                    <field.CheckboxField label="Initialize this app" />
                                  )}
                                </folderForm.AppField>
                              </div>

                              {service.enabled && (
                                <>
                                  <div className="grid gap-4 sm:grid-cols-2">
                                    <folderForm.AppField name={`services[${index}].domain`}>
                                      {(field) => (
                                        <field.TextField
                                          label="Domain"
                                          placeholder="my-app.localify"
                                        />
                                      )}
                                    </folderForm.AppField>

                                    <folderForm.AppField name={`services[${index}].target`}>
                                      {(field) => (
                                        <field.TextField
                                          label="Target"
                                          placeholder="3000"
                                          rightAction={
                                            <button
                                              type="button"
                                              onClick={() => void fillFolderPort(index)}
                                              disabled={availablePort.isPending}
                                              className="text-xs text-[--color-muted] hover:text-[--color-ink] disabled:cursor-not-allowed disabled:opacity-50"
                                            >
                                              {availablePort.isPending ? "Finding…" : "Free port"}
                                            </button>
                                          }
                                        />
                                      )}
                                    </folderForm.AppField>
                                  </div>

                                  {service.canUpdateDevScript && (
                                    <folderForm.AppField
                                      name={`services[${index}].updateDevScript`}
                                    >
                                      {(field) => (
                                        <field.CheckboxField
                                          label="Update package.json dev script"
                                          description="Rewrite the detected dev command to use the selected local port when possible."
                                        />
                                      )}
                                    </folderForm.AppField>
                                  )}
                                </>
                              )}
                            </div>
                          ))}
                        </div>
                      )}
                    </div>
                  );
                }}
              </folderForm.Field>

              {submitError && (
                <p role="alert" className="text-xs text-[--color-danger]">
                  {submitError}
                </p>
              )}

              <DialogFooter>
                <DialogClose render={<Button variant="outline">Cancel</Button>} />
                <folderForm.SubmitButton pendingLabel="Initializing…">
                  Create from folder
                </folderForm.SubmitButton>
              </DialogFooter>
            </form>
          </folderForm.AppForm>
        )}
      </DialogContent>
    </Dialog>
  );
};
