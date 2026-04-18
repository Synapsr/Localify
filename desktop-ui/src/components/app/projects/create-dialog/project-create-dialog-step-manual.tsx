"use client";

import { useMutation } from "@tanstack/react-query";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";

import { ChevronDownIcon, PlusIcon, XIcon } from "@/components/icons";
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
import { utilityMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";
import {
  emptyDomainEntry,
  normalizeSuffix,
  slugifyProjectName,
} from "@/lib/validators/validator.create-project";

import { DomainTargetFields } from "./domain-target-fields";
import { manualFormOpts } from "./form-options";

export const ProjectCreateDialogStepManual = withForm({
  ...manualFormOpts,
  props: {
    suffix: "",
    submitError: null as string | null,
    onBack: () => {},
  },
  render: function Render({ form, suffix, submitError, onBack }) {
    const availablePort = useMutation(utilityMutations.availablePort());
    const lastAutoSlugRef = useRef("");
    const normalizedSuffix = normalizeSuffix(suffix);
    const [expandedIndices, setExpandedIndices] = useState<Set<number>>(new Set([0]));

    const toggleExpanded = (index: number) => {
      setExpandedIndices((prev) => {
        const next = new Set(prev);
        if (next.has(index)) next.delete(index);
        else next.add(index);
        return next;
      });
    };

    const addDomain = (currentLength: number, push: (entry: ReturnType<typeof emptyDomainEntry>) => void) => {
      push(emptyDomainEntry());
      setExpandedIndices((prev) => new Set(prev).add(currentLength));
    };

    const removeDomain = (index: number, remove: (i: number) => void) => {
      remove(index);
      setExpandedIndices((prev) => {
        const next = new Set<number>();
        for (const idx of prev) {
          if (idx === index) continue;
          next.add(idx > index ? idx - 1 : idx);
        }
        return next;
      });
    };

    // Pre-fill the primary domain port on first mount so creating a basic
    // project is one click. The user can still edit it manually.
    useEffect(() => {
      const current = form.getFieldValue("domains")?.[0]?.target ?? "";
      if (current.trim().length > 0) return;

      let cancelled = false;
      availablePort
        .mutateAsync()
        .then(({ port }) => {
          if (cancelled) return;
          const stillEmpty = (form.getFieldValue("domains")?.[0]?.target ?? "").trim().length === 0;
          if (stillEmpty) {
            form.setFieldValue("domains[0].target", String(port));
          }
        })
        .catch(() => {
          // User can still press "Free port" manually.
        });

      return () => {
        cancelled = true;
      };
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    const fillPort = async (index: number) => {
      try {
        const { port } = await availablePort.mutateAsync();
        form.setFieldValue(`domains[${index}].target`, String(port));
      } catch {
        // Surface via submit flow if the user retries.
      }
    };

    return (
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
              Name your project and configure its first local domain.
            </DialogDescription>
          </DialogHeader>

          <DialogBody className="space-y-4">
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
              {(domainsField) => {
                const domains = domainsField.state.value;
                return (
                  <div className="space-y-2">
                    <div className="flex items-center justify-between">
                      <h3 className="text-sm font-medium text-foreground">Domains</h3>
                      <span className="text-xs text-muted-foreground">
                        {domains.length} configured
                      </span>
                    </div>

                    <div className="divide-y divide-border overflow-hidden rounded-xl border border-border">
                      {domains.map((entry, index) => {
                        const isExpanded = expandedIndices.has(index);
                        const label = index === 0 ? "Primary" : `Domain ${index + 1}`;
                        const previewDomain = entry.domain
                          ? `${entry.domain}${normalizedSuffix}`
                          : "";

                        return (
                          <div key={index} className="bg-background">
                            <div
                              className={cn(
                                "flex items-center gap-2 px-3 py-2 transition-colors hover:bg-muted/40",
                                isExpanded && "bg-muted/40",
                              )}
                            >
                              <button
                                type="button"
                                onClick={() => toggleExpanded(index)}
                                aria-expanded={isExpanded}
                                aria-controls={`domain-panel-${index}`}
                                className="flex min-w-0 flex-1 items-center gap-3 rounded-md text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
                              >
                                <div className="min-w-0 flex-1">
                                  <span className="text-sm font-medium text-foreground">
                                    {label}
                                  </span>
                                </div>

                                {(previewDomain || entry.target) && (
                                  <span className="hidden shrink-0 items-center gap-1.5 font-mono text-xs text-muted-foreground sm:flex">
                                    <span className="truncate">{previewDomain || "—"}</span>
                                    <span className="text-muted-foreground/60">→</span>
                                    <span>{entry.target || "—"}</span>
                                  </span>
                                )}

                                <ChevronDownIcon
                                  className={cn(
                                    "size-3.5 shrink-0 text-muted-foreground transition-transform duration-200",
                                    isExpanded && "rotate-180",
                                  )}
                                />
                              </button>

                              {index > 0 && (
                                <Button
                                  type="button"
                                  variant="ghost"
                                  size="icon-xs"
                                  onClick={() => removeDomain(index, domainsField.removeValue)}
                                  aria-label="Remove domain"
                                >
                                  <XIcon />
                                </Button>
                              )}
                            </div>

                            <AnimatePresence initial={false}>
                              {isExpanded && (
                                <motion.div
                                  key="panel"
                                  id={`domain-panel-${index}`}
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
                                      fields={`domains[${index}]`}
                                      domainSuffix={normalizedSuffix}
                                      onFillPort={() => void fillPort(index)}
                                      isFindingPort={availablePort.isPending}
                                    />

                                    <form.AppField name={`domains[${index}].wildcard`}>
                                      {(field) => (
                                        <field.CheckboxField label="Match all subdomains (wildcard)" />
                                      )}
                                    </form.AppField>
                                  </div>
                                </motion.div>
                              )}
                            </AnimatePresence>
                          </div>
                        );
                      })}
                    </div>

                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => addDomain(domains.length, domainsField.pushValue)}
                    >
                      <PlusIcon />
                      Add domain
                    </Button>
                  </div>
                );
              }}
            </form.Field>

            {submitError && (
              <p role="alert" className="text-xs text-destructive">
                {submitError}
              </p>
            )}
          </DialogBody>
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={onBack}>
              Back
            </Button>
            <div className="flex gap-2 sm:ml-auto">
              <DialogClose render={<Button variant="outline">Cancel</Button>} />
              <form.SubmitButton pendingLabel="Creating…">Create project</form.SubmitButton>
            </div>
          </DialogFooter>
        </form>
      </form.AppForm>
    );
  },
});
