"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";
import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";

import type { SettingsImportPreviewDto } from "@/types/roxy";

import {
  ArrowLeftIcon,
  ArrowUpDownIcon,
  BoxIcon,
  CheckIcon,
  DownloadIcon,
  FolderIcon,
  UploadIcon,
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
import { bootstrapQueries, systemMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { SettingsGroup, SettingsRow, SettingsSection } from "./settings-list";

type Step = "home" | "export" | "import";

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

const fileNameOf = (filePath: string) => {
  const parts = filePath.split(/[\\/]/);
  return parts[parts.length - 1] || filePath;
};

const stepTransition = { type: "spring", duration: 0.3, bounce: 0 } as const;

const slideVariants = {
  initial: (direction: number) => ({ opacity: 0, x: direction * 16 }),
  animate: { opacity: 1, x: 0 },
  exit: (direction: number) => ({ opacity: 0, x: direction * -16 }),
};

export const SettingsTransfer = () => {
  const { data } = useQuery(bootstrapQueries.read());
  const queryClient = useQueryClient();
  const exportMutation = useMutation(systemMutations.exportSettings());
  const previewImportMutation = useMutation(systemMutations.previewSettingsImport());
  const importMutation = useMutation(systemMutations.importSettings(queryClient));

  const [dialogOpen, setDialogOpen] = useState(false);
  const [step, setStep] = useState<Step>("home");
  const [direction, setDirection] = useState<1 | -1>(1);

  const [exportMode, setExportMode] = useState<"all" | "projects">("all");
  const [selectedExportProjectIds, setSelectedExportProjectIds] = useState<string[]>([]);
  const [importMode, setImportMode] = useState<"replaceAll" | "projects">("replaceAll");
  const [selectedImportProjectIds, setSelectedImportProjectIds] = useState<string[]>([]);
  const [preview, setPreview] = useState<SettingsImportPreviewDto | null>(null);
  const [exportResultMessage, setExportResultMessage] = useState<string | null>(null);
  const [importResultMessage, setImportResultMessage] = useState<string | null>(null);
  const [exportDialogError, setExportDialogError] = useState<string | null>(null);
  const [importDialogError, setImportDialogError] = useState<string | null>(null);

  if (!data) return null;

  const currentProjects = data.projects;
  const exportError = exportMutation.error instanceof Error ? exportMutation.error.message : null;
  const previewError =
    previewImportMutation.error instanceof Error ? previewImportMutation.error.message : null;
  const importError = importMutation.error instanceof Error ? importMutation.error.message : null;

  const resetAll = () => {
    setStep("home");
    setDirection(1);
    setExportMode("all");
    setSelectedExportProjectIds(currentProjects.map((project) => project.id));
    setImportMode("replaceAll");
    setSelectedImportProjectIds([]);
    setPreview(null);
    setExportResultMessage(null);
    setImportResultMessage(null);
    setExportDialogError(null);
    setImportDialogError(null);
    exportMutation.reset();
    previewImportMutation.reset();
    importMutation.reset();
  };

  const handleOpenChange = (nextOpen: boolean) => {
    setDialogOpen(nextOpen);
    if (!nextOpen) {
      resetAll();
    } else {
      setSelectedExportProjectIds(currentProjects.map((project) => project.id));
    }
  };

  const goTo = (next: Step) => {
    setDirection(next === "home" ? -1 : 1);
    setStep(next);
    if (next === "home") {
      setExportResultMessage(null);
      setImportResultMessage(null);
      setExportDialogError(null);
      setImportDialogError(null);
      exportMutation.reset();
      previewImportMutation.reset();
      importMutation.reset();
    }
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
    setExportDialogError(null);
    exportMutation.reset();

    try {
      const filePath = await saveFileDialog({
        defaultPath: exportFileName(exportMode),
        filters: [{ name: "Localify settings", extensions: ["json"] }],
      });

      if (typeof filePath !== "string") return;

      const selectedProjectIds = exportMode === "projects" ? selectedExportProjectIds : [];

      const result = await exportMutation.mutateAsync({
        filePath,
        mode: exportMode,
        projectIds: selectedProjectIds,
      });
      setExportResultMessage(
        `Saved ${result.projectCount} project${
          result.projectCount === 1 ? "" : "s"
        } to ${fileNameOf(result.filePath)}.`,
      );
    } catch (rawError) {
      console.error("Failed to open export dialog.", rawError);
      setExportDialogError(rawError instanceof Error ? rawError.message : String(rawError));
    }
  };

  const handleChooseImportFile = async () => {
    setImportResultMessage(null);
    setImportDialogError(null);
    previewImportMutation.reset();

    try {
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
    } catch (rawError) {
      console.error("Failed to open import dialog.", rawError);
      setImportDialogError(rawError instanceof Error ? rawError.message : String(rawError));
    }
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

  const canExport =
    !exportMutation.isPending &&
    !exportResultMessage &&
    !(exportMode === "projects" && selectedExportProjectIds.length === 0);
  const canImport =
    !importMutation.isPending &&
    !importResultMessage &&
    preview !== null &&
    !(importMode === "projects" && selectedImportProjectIds.length === 0);

  const title = step === "home" ? "Transfer settings" : step === "export" ? "Export" : "Import";
  const description =
    step === "home"
      ? "Move your Localify setup to another Mac, or share it with a teammate."
      : step === "export"
        ? "Save projects and portable settings to a file. Machine trust and the active license are not exported."
        : "Choose a previously exported file. Nothing is applied until you confirm.";

  return (
    <>
      <SettingsSection label="Transfer" icon={<ArrowUpDownIcon />}>
        <SettingsGroup>
          <SettingsRow
            icon={<ArrowUpDownIcon />}
            title="Import and export settings"
            description="Move your Localify setup to another Mac, or share selected projects with a teammate."
            onActivate={() => handleOpenChange(true)}
          />
        </SettingsGroup>
      </SettingsSection>

      <Dialog open={dialogOpen} onOpenChange={handleOpenChange}>
        <DialogContent className="max-h-[calc(100vh-2rem)] overflow-x-hidden overflow-y-auto overscroll-none sm:max-w-lg">
          <DialogHeader>
            <div className="flex items-center gap-1.5">
              {step !== "home" ? (
                <Button
                  variant="ghost"
                  size="icon-sm"
                  onClick={() => goTo("home")}
                  aria-label="Back"
                  className="-ml-1.5"
                >
                  <ArrowLeftIcon />
                </Button>
              ) : null}
              <DialogTitle className="text-base">{title}</DialogTitle>
            </div>
            <DialogDescription className="text-balance">{description}</DialogDescription>
          </DialogHeader>

          <DialogBody className="pb-2">
            <AnimatePresence mode="wait" initial={false} custom={direction}>
              {step === "home" ? (
                <motion.div
                  key="home"
                  custom={direction}
                  variants={slideVariants}
                  initial="initial"
                  animate="animate"
                  exit="exit"
                  transition={stepTransition}
                >
                  <HomeStep onChoose={(next) => goTo(next)} />
                </motion.div>
              ) : null}

              {step === "export" ? (
                <motion.div
                  key="export"
                  custom={direction}
                  variants={slideVariants}
                  initial="initial"
                  animate="animate"
                  exit="exit"
                  transition={stepTransition}
                  className="space-y-3"
                >
                  {exportResultMessage ? (
                    <ResultState
                      tone="success"
                      title="Export complete"
                      message={exportResultMessage}
                    />
                  ) : (
                    <>
                      <div className="space-y-2">
                        <ScopeTile
                          icon={<BoxIcon />}
                          title="Everything"
                          description="Every project, standalone domains, and portable settings."
                          selected={exportMode === "all"}
                          onSelect={() => setExportMode("all")}
                        />
                        <ScopeTile
                          icon={<FolderIcon />}
                          title="Selected projects"
                          description="Pick the projects to include. Global settings are skipped."
                          selected={exportMode === "projects"}
                          onSelect={() => setExportMode("projects")}
                          disabled={currentProjects.length === 0}
                        />
                      </div>

                      <AnimatePresence initial={false}>
                        {exportMode === "projects" && currentProjects.length > 0 ? (
                          <motion.div
                            key="export-projects"
                            initial={{ opacity: 0, height: 0 }}
                            animate={{ opacity: 1, height: "auto" }}
                            exit={{ opacity: 0, height: 0 }}
                            transition={stepTransition}
                            className="overflow-hidden"
                          >
                            <ProjectList
                              heading="Projects"
                              projects={currentProjects.map((project) => ({
                                id: project.id,
                                name: project.name,
                                enabled: project.enabled,
                                domainCount: project.domainCount,
                                hasRootPath: Boolean(project.rootPath),
                              }))}
                              selectedIds={selectedExportProjectIds}
                              onToggle={toggleExportProject}
                              onToggleAll={() =>
                                setSelectedExportProjectIds(
                                  exportAllSelected
                                    ? []
                                    : currentProjects.map((project) => project.id),
                                )
                              }
                              allSelected={exportAllSelected}
                            />
                          </motion.div>
                        ) : null}
                      </AnimatePresence>

                      {exportDialogError || exportError ? (
                        <p className="text-xs text-destructive">
                          {exportDialogError ?? exportError}
                        </p>
                      ) : null}
                    </>
                  )}
                </motion.div>
              ) : null}

              {step === "import" ? (
                <motion.div
                  key="import"
                  custom={direction}
                  variants={slideVariants}
                  initial="initial"
                  animate="animate"
                  exit="exit"
                  transition={stepTransition}
                  className="space-y-3"
                >
                  {importResultMessage ? (
                    <ResultState
                      tone="success"
                      title="Import complete"
                      message={importResultMessage}
                    />
                  ) : (
                    <>
                      {preview ? (
                        <FileCard
                          fileName={fileNameOf(preview.filePath)}
                          projectCount={preview.projectCount}
                          standaloneDomainCount={preview.standaloneDomainCount}
                          includesSettings={preview.includesSettings}
                          onChange={() => void handleChooseImportFile()}
                          loading={previewImportMutation.isPending}
                        />
                      ) : (
                        <FilePickerEmpty
                          onPick={() => void handleChooseImportFile()}
                          loading={previewImportMutation.isPending}
                        />
                      )}

                      <AnimatePresence initial={false}>
                        {preview && preview.includesSettings ? (
                          <motion.div
                            key="import-scope"
                            initial={{ opacity: 0, height: 0 }}
                            animate={{ opacity: 1, height: "auto" }}
                            exit={{ opacity: 0, height: 0 }}
                            transition={stepTransition}
                            className="space-y-2 overflow-hidden"
                          >
                            <ScopeTile
                              icon={<BoxIcon />}
                              title="Replace current setup"
                              description="Apply the file's portable settings and replace existing projects."
                              selected={importMode === "replaceAll"}
                              onSelect={() => setImportMode("replaceAll")}
                            />
                            <ScopeTile
                              icon={<FolderIcon />}
                              title="Only import projects"
                              description="Keep your current global settings and add selected projects."
                              selected={importMode === "projects"}
                              onSelect={() => setImportMode("projects")}
                            />
                          </motion.div>
                        ) : null}
                      </AnimatePresence>

                      <AnimatePresence initial={false}>
                        {preview && importMode === "projects" ? (
                          <motion.div
                            key="import-projects"
                            initial={{ opacity: 0, height: 0 }}
                            animate={{ opacity: 1, height: "auto" }}
                            exit={{ opacity: 0, height: 0 }}
                            transition={stepTransition}
                            className="overflow-hidden"
                          >
                            <ProjectList
                              heading="Projects in file"
                              projects={preview.projects.map((project) => ({
                                id: project.id,
                                name: project.name,
                                enabled: project.enabled,
                                domainCount: project.domainCount,
                                hasRootPath: project.hasRootPath,
                              }))}
                              selectedIds={selectedImportProjectIds}
                              onToggle={toggleImportProject}
                              onToggleAll={() =>
                                setSelectedImportProjectIds(
                                  importAllSelected
                                    ? []
                                    : preview.projects.map((project) => project.id),
                                )
                              }
                              allSelected={importAllSelected}
                            />
                          </motion.div>
                        ) : null}
                      </AnimatePresence>

                      {importDialogError || previewError ? (
                        <p className="text-xs text-destructive">
                          {importDialogError ?? previewError}
                        </p>
                      ) : null}
                      {importError ? (
                        <p className="text-xs text-destructive">{importError}</p>
                      ) : null}
                    </>
                  )}
                </motion.div>
              ) : null}
            </AnimatePresence>
          </DialogBody>

          <DialogFooter className="sticky right-0 bottom-0 left-0">
            {step !== "home" && !exportResultMessage && !importResultMessage ? (
              <Button variant="ghost" size="sm" onClick={() => goTo("home")}>
                <ArrowLeftIcon />
                Back
              </Button>
            ) : null}
            <div className="flex gap-2 sm:ml-auto">
              <Button variant="ghost" size="sm" onClick={() => handleOpenChange(false)}>
                {step === "home" || exportResultMessage || importResultMessage ? "Close" : "Cancel"}
              </Button>
              {step === "export" && !exportResultMessage ? (
                <Button
                  size="sm"
                  loading={exportMutation.isPending}
                  disabled={!canExport}
                  onClick={() => void handleExport()}
                >
                  <UploadIcon />
                  Export
                </Button>
              ) : null}
              {step === "import" && !importResultMessage ? (
                <Button
                  size="sm"
                  loading={importMutation.isPending}
                  disabled={!canImport}
                  onClick={() => void handleImport()}
                >
                  <DownloadIcon />
                  Import
                </Button>
              ) : null}
              {(step === "export" && exportResultMessage) ||
              (step === "import" && importResultMessage) ? (
                <Button size="sm" variant="outline" onClick={() => goTo("home")}>
                  Done
                </Button>
              ) : null}
            </div>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
};

const HomeStep = ({ onChoose }: { onChoose: (step: "export" | "import") => void }) => (
  <div className="grid gap-2 sm:grid-cols-2">
    <ActionTile
      icon={<UploadIcon />}
      title="Export"
      description="Save your projects and settings as a portable file."
      onClick={() => onChoose("export")}
    />
    <ActionTile
      icon={<DownloadIcon />}
      title="Import"
      description="Load projects and settings from a previously exported file."
      onClick={() => onChoose("import")}
    />
  </div>
);

const ActionTile = ({
  icon,
  title,
  description,
  onClick,
}: {
  icon: React.ReactNode;
  title: string;
  description: string;
  onClick: () => void;
}) => (
  <button
    type="button"
    onClick={onClick}
    className={cn(
      "group relative flex flex-col items-start gap-3 rounded-xl border border-border bg-background p-4 text-left",
      "transition-[transform,background-color,border-color,box-shadow] duration-150 ease-out",
      "hover:border-foreground/20 hover:bg-muted/40 hover:shadow-[0_1px_0_rgba(0,0,0,0.02),0_4px_16px_-8px_rgba(0,0,0,0.12)]",
      "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none",
      "active:scale-[0.98]",
    )}
  >
    <span className="flex size-9 items-center justify-center rounded-lg bg-muted text-foreground/80 transition-colors group-hover:bg-foreground group-hover:text-background [&>svg]:size-4">
      {icon}
    </span>
    <div className="space-y-1">
      <div className="text-sm font-medium text-foreground">{title}</div>
      <p className="text-xs leading-relaxed text-pretty text-muted-foreground">{description}</p>
    </div>
  </button>
);

const ScopeTile = ({
  icon,
  title,
  description,
  selected,
  onSelect,
  disabled,
}: {
  icon: React.ReactNode;
  title: string;
  description: string;
  selected: boolean;
  onSelect: () => void;
  disabled?: boolean;
}) => (
  <button
    type="button"
    onClick={onSelect}
    disabled={disabled}
    aria-pressed={selected}
    className={cn(
      "group flex w-full items-start gap-3 rounded-xl border px-3.5 py-3 text-left",
      "transition-[transform,background-color,border-color] duration-150 ease-out",
      "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none",
      "disabled:pointer-events-none disabled:opacity-50",
      selected
        ? "border-foreground/80 bg-muted/60"
        : "border-border bg-background hover:border-foreground/20 hover:bg-muted/40",
      !disabled && "active:scale-[0.99]",
    )}
  >
    <span
      className={cn(
        "mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-md transition-colors [&>svg]:size-3.5",
        selected ? "bg-foreground text-background" : "bg-muted text-foreground/70",
      )}
    >
      {icon}
    </span>
    <div className="min-w-0 flex-1 space-y-0.5">
      <div className="text-sm font-medium text-foreground">{title}</div>
      <p className="text-xs leading-relaxed text-pretty text-muted-foreground">{description}</p>
    </div>
    <AnimatePresence initial={false}>
      {selected ? (
        <motion.span
          key="check"
          initial={{ opacity: 0, scale: 0.25, filter: "blur(4px)" }}
          animate={{ opacity: 1, scale: 1, filter: "blur(0px)" }}
          exit={{ opacity: 0, scale: 0.25, filter: "blur(4px)" }}
          transition={stepTransition}
          className="mt-0.5 flex size-5 items-center justify-center rounded-full bg-foreground text-background [&>svg]:size-3"
        >
          <CheckIcon />
        </motion.span>
      ) : null}
    </AnimatePresence>
  </button>
);

type ListProject = {
  id: string;
  name: string;
  enabled: boolean;
  domainCount: number;
  hasRootPath: boolean;
};

const ProjectList = ({
  heading,
  projects,
  selectedIds,
  onToggle,
  onToggleAll,
  allSelected,
}: {
  heading: string;
  projects: ListProject[];
  selectedIds: string[];
  onToggle: (id: string) => void;
  onToggleAll: () => void;
  allSelected: boolean;
}) => (
  <div className="space-y-2 pt-2">
    <div className="flex items-center justify-between gap-3 px-0.5">
      <div className="flex items-baseline gap-2">
        <span className="text-[11px] font-medium tracking-wider text-muted-foreground uppercase">
          {heading}
        </span>
        <span className="text-[11px] text-muted-foreground/80 tabular-nums">
          {selectedIds.length}/{projects.length}
        </span>
      </div>
      {projects.length > 0 ? (
        <Button variant="ghost" size="xs" onClick={onToggleAll}>
          {allSelected ? "Clear" : "Select all"}
        </Button>
      ) : null}
    </div>
    <div className="max-h-60 overflow-y-auto rounded-xl border border-border bg-background">
      <ul className="divide-y divide-border/70">
        {projects.map((project) => {
          const checked = selectedIds.includes(project.id);
          return (
            <li key={project.id}>
              <label
                className={cn(
                  "flex cursor-pointer items-start gap-3 px-3 py-2.5 transition-colors",
                  "hover:bg-muted/40",
                )}
              >
                <input
                  type="checkbox"
                  checked={checked}
                  onChange={() => onToggle(project.id)}
                  className="mt-0.5 size-4 shrink-0 cursor-pointer rounded border-border accent-foreground"
                />
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <span className="truncate text-sm font-medium text-foreground">
                      {project.name}
                    </span>
                    {!project.enabled ? (
                      <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] tracking-wider text-muted-foreground uppercase">
                        Disabled
                      </span>
                    ) : null}
                  </div>
                  <p className="text-xs text-muted-foreground">
                    <span className="tabular-nums">{project.domainCount}</span> domain
                    {project.domainCount === 1 ? "" : "s"}
                    {project.hasRootPath ? " · includes folder path" : ""}
                  </p>
                </div>
              </label>
            </li>
          );
        })}
      </ul>
    </div>
  </div>
);

const FilePickerEmpty = ({ onPick, loading }: { onPick: () => void; loading: boolean }) => (
  <button
    type="button"
    onClick={onPick}
    disabled={loading}
    className={cn(
      "flex w-full flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-border bg-muted/20 px-4 py-8 text-center",
      "transition-[background-color,border-color,transform] duration-150 ease-out",
      "hover:border-foreground/30 hover:bg-muted/40",
      "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none",
      "disabled:pointer-events-none disabled:opacity-60",
      "active:scale-[0.99]",
    )}
  >
    <span className="flex size-9 items-center justify-center rounded-full bg-background text-foreground/80 shadow-[0_0_0_1px_rgba(0,0,0,0.06),0_1px_2px_rgba(0,0,0,0.04)] [&>svg]:size-4">
      <FolderIcon />
    </span>
    <div className="space-y-0.5">
      <div className="text-sm font-medium text-foreground">
        {loading ? "Reading file…" : "Choose a file"}
      </div>
      <p className="text-xs text-muted-foreground">Select a Localify export (.json)</p>
    </div>
  </button>
);

const FileCard = ({
  fileName,
  projectCount,
  standaloneDomainCount,
  includesSettings,
  onChange,
  loading,
}: {
  fileName: string;
  projectCount: number;
  standaloneDomainCount: number;
  includesSettings: boolean;
  onChange: () => void;
  loading: boolean;
}) => {
  const parts = [`${projectCount} project${projectCount === 1 ? "" : "s"}`];
  if (standaloneDomainCount > 0) {
    parts.push(
      `${standaloneDomainCount} standalone domain${standaloneDomainCount === 1 ? "" : "s"}`,
    );
  }
  if (includesSettings) {
    parts.push("portable settings");
  }

  return (
    <div className="flex items-center gap-3 rounded-xl border border-border bg-background p-3">
      <span className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted text-foreground/80 [&>svg]:size-4">
        <FolderIcon />
      </span>
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm font-medium text-foreground" title={fileName}>
          {fileName}
        </div>
        <p className="truncate text-xs text-muted-foreground">{parts.join(" · ")}</p>
      </div>
      <Button variant="ghost" size="sm" loading={loading} onClick={onChange} className="shrink-0">
        Change
      </Button>
    </div>
  );
};

const ResultState = ({
  tone,
  title,
  message,
}: {
  tone: "success";
  title: string;
  message: string;
}) => (
  <motion.div
    initial={{ opacity: 0, y: 4 }}
    animate={{ opacity: 1, y: 0 }}
    transition={stepTransition}
    className={cn(
      "flex items-start gap-3 rounded-xl border p-4",
      tone === "success" && "border-emerald-500/20 bg-emerald-500/[0.06]",
    )}
  >
    <span
      className={cn(
        "mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-full [&>svg]:size-3.5",
        tone === "success" && "bg-emerald-500 text-white",
      )}
    >
      <CheckIcon />
    </span>
    <div className="min-w-0 flex-1 space-y-0.5">
      <div className="text-sm font-medium text-foreground">{title}</div>
      <p className="text-xs leading-relaxed text-pretty text-muted-foreground">{message}</p>
    </div>
  </motion.div>
);
