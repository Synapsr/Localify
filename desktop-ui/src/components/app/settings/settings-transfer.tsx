"use client";

import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import {
  ArrowUpDownIcon,
  CheckIcon,
  FileTextIcon,
  FolderIcon,
} from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { bootstrapQueries, systemMutations } from "@/lib/queries";

import type { SettingsImportPreviewDto } from "@/types/roxy";

import { SettingsGroup, SettingsRow, SettingsSection } from "./settings-list";

const exportFileName = (mode: "all" | "projects") =>
  mode === "all" ? "localify-settings.json" : "localify-projects.json";

const importSummary = (result: {
  importedProjects: number;
  importedDomains: number;
  importedStandaloneDomains: number;
  appliedSettings: boolean;
  skippedRootPaths: number;
}) => {
  const parts = [
    `${result.importedProjects} project${result.importedProjects === 1 ? "" : "s"}`,
    `${result.importedDomains} project domain${result.importedDomains === 1 ? "" : "s"}`,
  ];

  if (result.importedStandaloneDomains > 0) {
    parts.push(
      `${result.importedStandaloneDomains} standalone domain${
        result.importedStandaloneDomains === 1 ? "" : "s"
      }`,
    );
  }

  if (result.appliedSettings) {
    parts.push("portable settings");
  }

  if (result.skippedRootPaths > 0) {
    parts.push(
      `${result.skippedRootPaths} project folder path${
        result.skippedRootPaths === 1 ? "" : "s"
      } skipped`,
    );
  }

  return `Imported ${parts.join(", ")}.`;
};

const previewScopeLabel = (preview: SettingsImportPreviewDto) =>
  preview.includesSettings
    ? "Full machine snapshot"
    : preview.scope === "projects"
      ? "Project-only snapshot"
      : "Project snapshot";

export const SettingsTransfer = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const exportMutation = useMutation(systemMutations.exportSettings());
  const previewImportMutation = useMutation(systemMutations.previewSettingsImport());
  const importMutation = useMutation(systemMutations.importSettings(queryClient));
  const [dialogOpen, setDialogOpen] = useState(false);
  const [exportMode, setExportMode] = useState<"all" | "projects">("all");
  const [selectedExportProjectIds, setSelectedExportProjectIds] = useState<string[]>([]);
  const [importMode, setImportMode] = useState<"replaceAll" | "projects">("replaceAll");
  const [selectedImportProjectIds, setSelectedImportProjectIds] = useState<string[]>([]);
  const [preview, setPreview] = useState<SettingsImportPreviewDto | null>(null);
  const [exportResultMessage, setExportResultMessage] = useState<string | null>(null);
  const [importResultMessage, setImportResultMessage] = useState<string | null>(null);

  if (!data) return null;

  const currentProjects = data.projects;
  const exportError =
    exportMutation.error instanceof Error ? exportMutation.error.message : null;
  const previewError =
    previewImportMutation.error instanceof Error
      ? previewImportMutation.error.message
      : null;
  const importError =
    importMutation.error instanceof Error ? importMutation.error.message : null;

  const resetDialogState = (nextOpen: boolean) => {
    setDialogOpen(nextOpen);
    if (!nextOpen) {
      setExportMode("all");
      setSelectedExportProjectIds([]);
      setImportMode("replaceAll");
      setSelectedImportProjectIds([]);
      setPreview(null);
      setExportResultMessage(null);
      setImportResultMessage(null);
      exportMutation.reset();
      previewImportMutation.reset();
      importMutation.reset();
      return;
    }

    setSelectedExportProjectIds(currentProjects.map((project) => project.id));
    setExportResultMessage(null);
    setImportResultMessage(null);
  };

  const toggleExportProject = (projectId: string) => {
    setSelectedExportProjectIds((current) =>
      current.includes(projectId)
        ? current.filter((candidate) => candidate !== projectId)
        : [...current, projectId],
    );
  };

  const toggleImportProject = (projectId: string) => {
    setSelectedImportProjectIds((current) =>
      current.includes(projectId)
        ? current.filter((candidate) => candidate !== projectId)
        : [...current, projectId],
    );
  };

  const exportAllSelected = selectedExportProjectIds.length === currentProjects.length;
  const importAllSelected =
    preview !== null && selectedImportProjectIds.length === preview.projects.length;

  const handleExport = async () => {
    setExportResultMessage(null);
    const filePath = await saveFileDialog({
      defaultPath: exportFileName(exportMode),
      filters: [{ name: "Localify settings", extensions: ["json"] }],
    });

    if (typeof filePath !== "string") return;

    const selectedProjectIds =
      exportMode === "projects" ? selectedExportProjectIds : [];

    const result = await exportMutation.mutateAsync({
      filePath,
      mode: exportMode,
      projectIds: selectedProjectIds,
    });
    setExportResultMessage(
      `Exported ${result.projectCount} project${
        result.projectCount === 1 ? "" : "s"
      } to ${result.filePath}.`,
    );
  };

  const handleChooseImportFile = async () => {
    setImportResultMessage(null);
    const filePath = await openFileDialog({
      multiple: false,
      directory: false,
      filters: [{ name: "Localify settings", extensions: ["json"] }],
    });

    if (typeof filePath !== "string") return;

    const nextPreview = await previewImportMutation.mutateAsync({ filePath });
    setPreview(nextPreview);
    setImportMode(nextPreview.includesSettings ? "replaceAll" : "projects");
    setSelectedImportProjectIds(nextPreview.projects.map((project) => project.id));
  };

  const handleImport = async () => {
    if (!preview) return;

    setImportResultMessage(null);
    const result = await importMutation.mutateAsync({
      filePath: preview.filePath,
      mode: importMode,
      projectIds: importMode === "projects" ? selectedImportProjectIds : [],
    });

    const summary = importSummary(result);
    const reloadMessage = result.daemonReloadError
      ? ` ${result.daemonReloadError}`
      : result.daemonReloaded
        ? " The daemon was reloaded."
        : "";

    setImportResultMessage(`${summary}${reloadMessage}`);
  };

  return (
    <>
      <SettingsSection label="Transfer" icon={<ArrowUpDownIcon />}>
        <SettingsGroup>
          <SettingsRow
            icon={<ArrowUpDownIcon />}
            title="Import and export settings"
            description="Move your Localify setup to another Mac, or share selected projects with a teammate."
            onActivate={() => resetDialogState(true)}
          />
        </SettingsGroup>
      </SettingsSection>

      <Dialog open={dialogOpen} onOpenChange={resetDialogState}>
        <DialogContent className="sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>Import and export settings</DialogTitle>
            <DialogDescription>
              Portable exports include projects, domains, ports, launcher preferences, and project
              folders when they are known. Machine trust and the active license are not exported.
            </DialogDescription>
          </DialogHeader>

          <DialogBody className="space-y-6 pb-2">
            <section className="space-y-3 rounded-xl border border-[--color-border]/70 bg-neutral-50/60 p-4 dark:bg-neutral-800/60">
              <div className="space-y-1">
                <h3 className="text-sm font-medium text-[--color-ink]">Export</h3>
                <p className="text-xs text-[--color-muted]">
                  Create a portable file with every importable Localify setting, or only the
                  projects you choose.
                </p>
              </div>

              <div className="grid gap-2 sm:grid-cols-2">
                <button
                  type="button"
                  onClick={() => setExportMode("all")}
                  className={`rounded-lg border px-3 py-3 text-left transition-colors ${
                    exportMode === "all"
                      ? "border-[--color-ink] bg-[--color-border]/20"
                      : "border-[--color-border] bg-[--color-surface]"
                  }`}
                >
                  <div className="text-sm font-medium text-[--color-ink]">Everything</div>
                  <p className="mt-1 text-xs text-[--color-muted]">
                    All portable settings, every project, and standalone domains.
                  </p>
                </button>
                <button
                  type="button"
                  onClick={() => setExportMode("projects")}
                  className={`rounded-lg border px-3 py-3 text-left transition-colors ${
                    exportMode === "projects"
                      ? "border-[--color-ink] bg-[--color-border]/20"
                      : "border-[--color-border] bg-[--color-surface]"
                  }`}
                >
                  <div className="text-sm font-medium text-[--color-ink]">Selected projects</div>
                  <p className="mt-1 text-xs text-[--color-muted]">
                    Only the projects you tick below, without the global settings.
                  </p>
                </button>
              </div>

              {exportMode === "projects" ? (
                <div className="space-y-2 rounded-lg border border-[--color-border] bg-[--color-surface] p-3">
                  <div className="flex items-center justify-between gap-3">
                    <p className="text-xs text-[--color-muted]">
                      {selectedExportProjectIds.length} of {currentProjects.length} selected
                    </p>
                    {currentProjects.length > 0 ? (
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() =>
                          setSelectedExportProjectIds(
                            exportAllSelected ? [] : currentProjects.map((project) => project.id),
                          )
                        }
                      >
                        {exportAllSelected ? "Clear all" : "Select all"}
                      </Button>
                    ) : null}
                  </div>
                  <div className="max-h-52 space-y-2 overflow-y-auto">
                    {currentProjects.map((project) => {
                      const checked = selectedExportProjectIds.includes(project.id);

                      return (
                        <label
                          key={project.id}
                          className="flex cursor-pointer items-start gap-3 rounded-lg border border-[--color-border]/70 px-3 py-2"
                        >
                          <input
                            type="checkbox"
                            checked={checked}
                            onChange={() => toggleExportProject(project.id)}
                            className="mt-0.5 h-4 w-4 rounded border-[--color-border] accent-[--color-ink]"
                          />
                          <div className="min-w-0 flex-1">
                            <div className="flex items-center gap-2">
                              <span className="text-sm font-medium text-[--color-ink]">
                                {project.name}
                              </span>
                              {!project.enabled ? (
                                <span className="text-[10px] uppercase tracking-wider text-[--color-muted]">
                                  Disabled
                                </span>
                              ) : null}
                            </div>
                            <p className="text-xs text-[--color-muted]">
                              {project.domainCount} domain{project.domainCount === 1 ? "" : "s"}
                              {project.rootPath ? " · includes folder path" : ""}
                            </p>
                          </div>
                        </label>
                      );
                    })}
                  </div>
                </div>
              ) : null}

              {exportResultMessage ? (
                <p className="text-xs text-emerald-600">{exportResultMessage}</p>
              ) : null}
              {exportError ? (
                <p className="text-xs text-[--color-danger]">{exportError}</p>
              ) : null}

              <div className="flex justify-end">
                <Button
                  size="sm"
                  loading={exportMutation.isPending}
                  disabled={
                    exportMode === "projects" && selectedExportProjectIds.length === 0
                  }
                  onClick={() => void handleExport()}
                >
                  <FileTextIcon />
                  Export
                </Button>
              </div>
            </section>

            <section className="space-y-3 rounded-xl border border-[--color-border]/70 bg-neutral-50/60 p-4 dark:bg-neutral-800/60">
              <div className="space-y-1">
                <h3 className="text-sm font-medium text-[--color-ink]">Import</h3>
                <p className="text-xs text-[--color-muted]">
                  Load a file first, then either replace the current setup or import only the
                  projects you want.
                </p>
              </div>

              <div className="flex flex-wrap items-center gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  loading={previewImportMutation.isPending}
                  onClick={() => void handleChooseImportFile()}
                >
                  <FolderIcon />
                  Choose file
                </Button>
                <Input
                  value={preview?.filePath ?? ""}
                  readOnly
                  placeholder="No file selected"
                  className="min-w-0 flex-1"
                />
              </div>

              {preview ? (
                <div className="space-y-3 rounded-lg border border-[--color-border] bg-[--color-surface] p-3">
                  <div className="space-y-1">
                    <div className="flex items-center gap-2">
                      <CheckIcon className="size-4 text-emerald-600" />
                      <span className="text-sm font-medium text-[--color-ink]">
                        {previewScopeLabel(preview)}
                      </span>
                    </div>
                    <p className="text-xs text-[--color-muted]">
                      Version {preview.version} · {preview.projectCount} project
                      {preview.projectCount === 1 ? "" : "s"}
                      {preview.standaloneDomainCount > 0
                        ? ` · ${preview.standaloneDomainCount} standalone domain${
                            preview.standaloneDomainCount === 1 ? "" : "s"
                          }`
                        : ""}
                    </p>
                  </div>

                  {preview.includesSettings ? (
                    <div className="grid gap-2 sm:grid-cols-2">
                      <button
                        type="button"
                        onClick={() => setImportMode("replaceAll")}
                        className={`rounded-lg border px-3 py-3 text-left transition-colors ${
                          importMode === "replaceAll"
                            ? "border-[--color-ink] bg-[--color-border]/20"
                            : "border-[--color-border] bg-[--color-surface]"
                        }`}
                      >
                        <div className="text-sm font-medium text-[--color-ink]">
                          Replace current setup
                        </div>
                        <p className="mt-1 text-xs text-[--color-muted]">
                          Apply the imported portable settings and replace your current projects.
                        </p>
                      </button>
                      <button
                        type="button"
                        onClick={() => setImportMode("projects")}
                        className={`rounded-lg border px-3 py-3 text-left transition-colors ${
                          importMode === "projects"
                            ? "border-[--color-ink] bg-[--color-border]/20"
                            : "border-[--color-border] bg-[--color-surface]"
                        }`}
                      >
                        <div className="text-sm font-medium text-[--color-ink]">
                          Import selected projects
                        </div>
                        <p className="mt-1 text-xs text-[--color-muted]">
                          Keep your current global settings and only bring in chosen projects.
                        </p>
                      </button>
                    </div>
                  ) : null}

                  {importMode === "projects" ? (
                    <div className="space-y-2">
                      <div className="flex items-center justify-between gap-3">
                        <p className="text-xs text-[--color-muted]">
                          {selectedImportProjectIds.length} of {preview.projects.length} selected
                        </p>
                        {preview.projects.length > 0 ? (
                          <Button
                            variant="ghost"
                            size="sm"
                            onClick={() =>
                              setSelectedImportProjectIds(
                                importAllSelected
                                  ? []
                                  : preview.projects.map((project) => project.id),
                              )
                            }
                          >
                            {importAllSelected ? "Clear all" : "Select all"}
                          </Button>
                        ) : null}
                      </div>
                      <div className="max-h-52 space-y-2 overflow-y-auto">
                        {preview.projects.map((project) => {
                          const checked = selectedImportProjectIds.includes(project.id);

                          return (
                            <label
                              key={project.id}
                              className="flex cursor-pointer items-start gap-3 rounded-lg border border-[--color-border]/70 px-3 py-2"
                            >
                              <input
                                type="checkbox"
                                checked={checked}
                                onChange={() => toggleImportProject(project.id)}
                                className="mt-0.5 h-4 w-4 rounded border-[--color-border] accent-[--color-ink]"
                              />
                              <div className="min-w-0 flex-1">
                                <div className="flex items-center gap-2">
                                  <span className="text-sm font-medium text-[--color-ink]">
                                    {project.name}
                                  </span>
                                  {!project.enabled ? (
                                    <span className="text-[10px] uppercase tracking-wider text-[--color-muted]">
                                      Disabled
                                    </span>
                                  ) : null}
                                </div>
                                <p className="text-xs text-[--color-muted]">
                                  {project.domainCount} domain
                                  {project.domainCount === 1 ? "" : "s"}
                                  {project.hasRootPath ? " · includes folder path" : ""}
                                </p>
                              </div>
                            </label>
                          );
                        })}
                      </div>
                    </div>
                  ) : null}
                </div>
              ) : null}

              {importResultMessage ? (
                <p className="text-xs text-emerald-600">{importResultMessage}</p>
              ) : null}
              {previewError ? (
                <p className="text-xs text-[--color-danger]">{previewError}</p>
              ) : null}
              {importError ? (
                <p className="text-xs text-[--color-danger]">{importError}</p>
              ) : null}
            </section>
          </DialogBody>

          <DialogFooter>
            <Button variant="ghost" size="sm" onClick={() => resetDialogState(false)}>
              Close
            </Button>
            <Button
              size="sm"
              loading={importMutation.isPending}
              disabled={
                !preview ||
                (importMode === "projects" && selectedImportProjectIds.length === 0)
              }
              onClick={() => void handleImport()}
            >
              <ArrowUpDownIcon />
              Import
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
};
