"use client";

import { useMutation } from "@tanstack/react-query";
import { open as openDirectoryDialog } from "@tauri-apps/plugin-dialog";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";

import { ChevronDownIcon, EditIcon, FolderIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  DialogBody,
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { withForm } from "@/hooks/form/form";
import { projectMutations, utilityMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";
import { normalizeSuffix } from "@/lib/validators/validator.create-project";

import { DomainTargetFields } from "./domain-target-fields";
import { folderFormOpts } from "./form-options";

export const ProjectCreateDialogStepFolder = withForm({
  ...folderFormOpts,
  props: {
    suffix: "",
    initialRootPath: null as string | null,
    submitError: null as string | null,
    onBack: () => {},
  },
  render: function Render({ form, suffix, initialRootPath, submitError, onBack }) {
    const normalizedSuffix = normalizeSuffix(suffix);
    const detectInitPlan = useMutation(projectMutations.detectInitPlan());
    const availablePort = useMutation(utilityMutations.availablePort());
    const [selectedFolderPath, setSelectedFolderPath] = useState(initialRootPath ?? "");
    const [folderError, setFolderError] = useState<string | null>(null);
    const [expandedIds, setExpandedIds] = useState<Set<string>>(new Set());

    const toggleExpanded = (id: string) => {
      setExpandedIds((prev) => {
        const next = new Set(prev);
        if (next.has(id)) next.delete(id);
        else next.add(id);
        return next;
      });
    };

    const detectAndLoadPlan = async (path: string) => {
      setFolderError(null);
      setSelectedFolderPath(path);
      form.setFieldValue("rootPath", path);
      form.setFieldValue("name", "");
      form.setFieldValue("services", []);
      setExpandedIds(new Set());

      try {
        const plan = await detectInitPlan.mutateAsync({ rootPath: path });
        form.setFieldValue("rootPath", plan.rootPath);
        form.setFieldValue("name", plan.name);
        form.setFieldValue(
          "services",
          plan.services.map((service) => ({
            ...service,
            domain: stripSuffix(service.domain, normalizedSuffix),
          })),
        );
      } catch (rawError) {
        const message = rawError instanceof Error ? rawError.message : String(rawError);
        setFolderError(message);
      }
    };

    useEffect(() => {
      if (initialRootPath) {
        void detectAndLoadPlan(initialRootPath);
      }
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [initialRootPath]);

    const selectFolder = async () => {
      try {
        const selected = await openDirectoryDialog({
          directory: true,
          multiple: false,
          title: "Select project folder",
        });
        if (typeof selected !== "string") return;
        await detectAndLoadPlan(selected);
      } catch (rawError) {
        const message = rawError instanceof Error ? rawError.message : String(rawError);
        setFolderError(message);
      }
    };

    const fillPort = async (index: number) => {
      try {
        const { port } = await availablePort.mutateAsync();
        form.setFieldValue(`services[${index}].target`, String(port));
      } catch {
        // Surfaced via submit flow if the user retries.
      }
    };

    const errorMessage = submitError ?? folderError;
    const hasFolder = selectedFolderPath.length > 0;

    return (
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
            <DialogTitle>From folder</DialogTitle>
            <DialogDescription>
              Pick a folder, review the detected apps, and initialize them in one go.
            </DialogDescription>
          </DialogHeader>

          <DialogBody className="space-y-4">
            {hasFolder ? (
              <div className="flex items-center gap-2 rounded-lg border border-border bg-muted/30 py-1.5 pr-1.5 pl-3">
                <FolderIcon className="size-3.5 shrink-0 text-muted-foreground" />
                <span
                  className="min-w-0 flex-1 truncate font-mono text-xs text-foreground"
                  title={selectedFolderPath}
                >
                  {selectedFolderPath}
                </span>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  loading={detectInitPlan.isPending}
                  onClick={() => void selectFolder()}
                  aria-label="Change folder"
                >
                  <EditIcon />
                </Button>
              </div>
            ) : (
              <Button
                type="button"
                variant="outline"
                loading={detectInitPlan.isPending}
                onClick={() => void selectFolder()}
                className="w-full justify-start gap-2 font-normal text-muted-foreground"
              >
                <FolderIcon className="size-4" />
                Choose a local folder to initialize…
              </Button>
            )}

            <form.AppField name="name">
              {(field) => <field.TextField label="Project name" placeholder="my-app" />}
            </form.AppField>

            <form.Field name="services" mode="array">
              {(servicesField) => {
                const services = servicesField.state.value;
                const enabledCount = services.filter((service) => service.enabled).length;

                if (services.length === 0) {
                  return (
                    <div className="rounded-xl border border-dashed border-border px-4 py-6 text-center text-sm text-muted-foreground">
                      {detectInitPlan.isPending
                        ? "Inspecting folder…"
                        : hasFolder
                          ? "No apps detected in this folder."
                          : "Select a folder to inspect its local apps and suggested domains."}
                    </div>
                  );
                }

                return (
                  <div className="space-y-2">
                    <div className="flex items-center justify-between">
                      <h3 className="text-sm font-medium text-foreground">Detected apps</h3>
                      <span className="text-xs text-muted-foreground">
                        {enabledCount} of {services.length} selected
                      </span>
                    </div>

                    <div className="divide-y divide-border overflow-hidden rounded-xl border border-border">
                      {services.map((service, index) => {
                        const isExpanded = expandedIds.has(service.id);
                        return (
                          <div
                            key={service.id}
                            className={cn(
                              "bg-background transition-colors",
                              !service.enabled && "bg-muted/20",
                            )}
                          >
                            <div
                              className={cn(
                                "flex items-center gap-3 px-3 py-2 transition-colors hover:bg-muted/40",
                                isExpanded && "bg-muted/40",
                              )}
                            >
                              <form.AppField name={`services[${index}].enabled`}>
                                {(field) => (
                                  <input
                                    type="checkbox"
                                    checked={field.state.value ?? false}
                                    onChange={(event) => field.handleChange(event.target.checked)}
                                    onBlur={field.handleBlur}
                                    aria-label={`Initialize ${service.label}`}
                                    className="size-4 shrink-0 rounded border-border accent-foreground"
                                  />
                                )}
                              </form.AppField>

                              <button
                                type="button"
                                onClick={() => toggleExpanded(service.id)}
                                aria-expanded={isExpanded}
                                aria-controls={`service-panel-${service.id}`}
                                className={cn(
                                  "flex min-w-0 flex-1 items-center gap-3 rounded-md text-left outline-none",
                                  "focus-visible:ring-2 focus-visible:ring-ring/50",
                                  !service.enabled && "opacity-60",
                                )}
                              >
                                <div className="min-w-0 flex-1">
                                  <div className="flex items-baseline gap-2">
                                    <span className="truncate text-sm font-medium text-foreground">
                                      {service.label}
                                    </span>
                                    <span className="truncate font-mono text-xs text-muted-foreground">
                                      {service.displayPath}
                                    </span>
                                  </div>
                                </div>

                                <ServicePreview
                                  domain={
                                    service.domain ? `${service.domain}${normalizedSuffix}` : ""
                                  }
                                  target={service.target}
                                />

                                <ChevronDownIcon
                                  className={cn(
                                    "size-3.5 shrink-0 text-muted-foreground transition-transform duration-200",
                                    isExpanded && "rotate-180",
                                  )}
                                />
                              </button>
                            </div>

                            <AnimatePresence initial={false}>
                              {isExpanded && (
                                <motion.div
                                  key="panel"
                                  id={`service-panel-${service.id}`}
                                  initial={{ height: 0, opacity: 0 }}
                                  animate={{ height: "auto", opacity: 1 }}
                                  exit={{ height: 0, opacity: 0 }}
                                  transition={{
                                    height: { duration: 0.22, ease: [0.16, 1, 0.3, 1] },
                                    opacity: { duration: 0.18, ease: "easeOut" },
                                  }}
                                  style={{ overflow: "hidden" }}
                                >
                                  <div className="space-y-3 border-t border-border bg-muted/20 px-3 py-3">
                                    <DomainTargetFields
                                      form={form}
                                      fields={`services[${index}]`}
                                      domainSuffix={normalizedSuffix}
                                      onFillPort={() => void fillPort(index)}
                                      isFindingPort={availablePort.isPending}
                                    />

                                    {service.canUpdateDevScript && (
                                      <form.AppField
                                        name={`services[${index}].updateDevScript`}
                                      >
                                        {(field) => (
                                          <field.CheckboxField
                                            label="Update package.json dev script"
                                            description="Rewrite the detected dev command to use the selected local port when possible."
                                          />
                                        )}
                                      </form.AppField>
                                    )}
                                  </div>
                                </motion.div>
                              )}
                            </AnimatePresence>
                          </div>
                        );
                      })}
                    </div>
                  </div>
                );
              }}
            </form.Field>

            {errorMessage && (
              <p role="alert" className="text-xs text-destructive">
                {errorMessage}
              </p>
            )}
          </DialogBody>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onBack}>
              Back
            </Button>
            <div className="flex gap-2 sm:ml-auto">
              <DialogClose render={<Button variant="outline">Cancel</Button>} />
              <form.SubmitButton pendingLabel="Initializing…">Create from folder</form.SubmitButton>
            </div>
          </DialogFooter>
        </form>
      </form.AppForm>
    );
  },
});

function stripSuffix(value: string, suffix: string): string {
  return suffix && value.endsWith(suffix) ? value.slice(0, -suffix.length) : value;
}

function ServicePreview({ domain, target }: { domain: string; target: string }) {
  if (!domain && !target) return null;

  return (
    <span className="hidden shrink-0 items-center gap-1.5 font-mono text-xs text-muted-foreground sm:flex">
      <span className="truncate">{domain || "—"}</span>
      <span className="text-muted-foreground/60">→</span>
      <span>{target || "—"}</span>
    </span>
  );
}
