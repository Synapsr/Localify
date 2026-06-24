"use client";

import { revalidateLogic } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open as openDirectoryDialog } from "@tauri-apps/plugin-dialog";
import { type PropsWithChildren, useEffect, useState } from "react";

import type { CreateProjectDomainRequest } from "@/types/roxy";

import { Dialog, DialogContent } from "@/components/ui/dialog";
import { useAppForm } from "@/hooks/form/form";
import { captureDesktopEvent } from "@/lib/analytics";
import { bootstrapQueries, projectMutations } from "@/lib/queries";
import {
  createProjectSchema,
  normalizeDomain,
  normalizeSuffix,
} from "@/lib/validators/validator.create-project";
import { initProjectFromFolderSchema } from "@/lib/validators/validator.init-project-from-folder";

import { folderFormOpts, manualFormOpts } from "./create-dialog/form-options";
import { ProjectCreateDialogStepFolder } from "./create-dialog/project-create-dialog-step-folder";
import { ProjectCreateDialogStepManual } from "./create-dialog/project-create-dialog-step-manual";
import {
  ProjectCreateDialogStepMode,
  type ProjectCreationMode,
} from "./create-dialog/project-create-dialog-step-mode";
import {
  consumeQueuedProjectCreateDialogOpen,
  subscribeToProjectCreateDialogOpenRequest,
} from "./util.project-create-dialog";

type Step = "mode" | ProjectCreationMode;

export const ProjectsCreateDialog = ({ children }: PropsWithChildren) => {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [step, setStep] = useState<Step>("mode");
  const [submitError, setSubmitError] = useState<string | null>(null);
  const [folderInitPath, setFolderInitPath] = useState<string | null>(null);

  const createProject = useMutation(projectMutations.create(queryClient));
  const initFromFolder = useMutation(projectMutations.initFromFolder(queryClient));

  const { data } = useQuery(bootstrapQueries.read());
  const suffix = data?.environment.fixedDomainSuffix ?? "";
  const normalizedSuffix = normalizeSuffix(suffix);

  const manualForm = useAppForm({
    ...manualFormOpts,
    validationLogic: revalidateLogic({ mode: "submit", modeAfterSubmission: "change" }),
    validators: { onDynamic: createProjectSchema },
    onSubmit: async ({ value }) => {
      setSubmitError(null);

      try {
        const domains: CreateProjectDomainRequest[] = value.domains
          .filter((entry, index) => {
            if (index === 0) return entry.domain.length > 0 || entry.target.length > 0;
            return true;
          })
          .map((entry) => ({
            domain: normalizeDomain(entry.domain, normalizedSuffix),
            wildcard: entry.wildcard,
            routes: [entry.target],
          }));

        await createProject.mutateAsync({ name: value.name, domains });
        captureDesktopEvent("desktop_project_created", {
          mode: "manual",
          domain_count: domains.length,
        });
        closeAndReset();
      } catch (rawError) {
        setSubmitError(rawError instanceof Error ? rawError.message : String(rawError));
      }
    },
  });

  const folderForm = useAppForm({
    ...folderFormOpts,
    validationLogic: revalidateLogic({ mode: "submit", modeAfterSubmission: "change" }),
    validators: { onDynamic: initProjectFromFolderSchema },
    onSubmit: async ({ value }) => {
      setSubmitError(null);

      try {
        await initFromFolder.mutateAsync({
          rootPath: value.rootPath,
          name: value.name,
          services: value.services.map((service) => ({
            id: service.id,
            domain: normalizeDomain(service.domain, normalizedSuffix),
            target: service.target,
            enabled: service.enabled,
            updateDevScript: service.updateDevScript,
            serviceName: service.serviceName,
            servicePath: service.servicePath,
            command: service.command,
          })),
        });
        captureDesktopEvent("desktop_project_created", {
          mode: "folder",
          service_count: value.services.length,
          enabled_service_count: value.services.filter((service) => service.enabled).length,
        });
        closeAndReset();
      } catch (rawError) {
        setSubmitError(rawError instanceof Error ? rawError.message : String(rawError));
      }
    },
  });

  useEffect(() => {
    const openDialog = () => setOpen(true);
    if (consumeQueuedProjectCreateDialogOpen()) openDialog();
    return subscribeToProjectCreateDialogOpenRequest(openDialog);
  }, []);

  const resetState = () => {
    setStep("mode");
    setSubmitError(null);
    setFolderInitPath(null);
    manualForm.reset();
    folderForm.reset();
  };

  const handleModeSelect = async (mode: ProjectCreationMode) => {
    if (mode === "manual") {
      setStep("manual");
      return;
    }
    try {
      const selected = await openDirectoryDialog({
        directory: true,
        multiple: false,
        title: "Select project folder",
      });
      if (typeof selected !== "string") return;
      setFolderInitPath(selected);
      setStep("folder");
    } catch {
      // Native dialog errors are rare; surface via the step's own flow if retried.
    }
  };

  const closeAndReset = () => {
    resetState();
    setOpen(false);
  };

  const goBack = () => {
    setSubmitError(null);
    setStep("mode");
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => {
        setOpen(nextOpen);
        if (!nextOpen) resetState();
      }}
    >
      {children}
      <DialogContent className="max-h-[calc(100vh-2rem)] overflow-y-auto overscroll-none sm:max-w-2xl">
        {step === "mode" && (
          <ProjectCreateDialogStepMode onSelect={(mode) => void handleModeSelect(mode)} />
        )}

        {step === "manual" && (
          <ProjectCreateDialogStepManual
            form={manualForm}
            suffix={suffix}
            submitError={submitError}
            onBack={goBack}
          />
        )}

        {step === "folder" && (
          <ProjectCreateDialogStepFolder
            form={folderForm}
            suffix={suffix}
            initialRootPath={folderInitPath}
            submitError={submitError}
            onBack={goBack}
          />
        )}
      </DialogContent>
    </Dialog>
  );
};
