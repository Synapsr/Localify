"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";

import type {
  EnvironmentFileCopyItemDto,
  EnvironmentFileCopyRequest,
  EnvironmentFileCopyStatusDto,
  EnvironmentFileRequest,
} from "@/types/localify";

import { ArrowRightIcon, CopyIcon, FileTextIcon, SpinnerIcon } from "@/components/icons";
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
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { projectMutations, projectQueries } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { ProjectEnvironmentFileDialog } from "./project-environment-file-dialog";

interface ProjectEnvironmentFileCopyDialogProps {
  open: boolean;
  projectId: string;
  targetEnvironmentSlug: string;
  targetLabel?: string;
  sourceEnvironmentSlug?: string;
  onOpenChange: (open: boolean) => void;
}

const statusLabel: Record<EnvironmentFileCopyStatusDto, string> = {
  create: "Create",
  overwrite: "Overwrite",
  skipExists: "Exists",
  blocked: "Blocked",
  copied: "Imported",
  skipped: "Skipped",
};

const statusClassName: Record<EnvironmentFileCopyStatusDto, string> = {
  create: "border-emerald-500/20 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  overwrite: "border-amber-500/20 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  skipExists: "border-border bg-muted/60 text-muted-foreground",
  blocked: "border-destructive/20 bg-destructive/10 text-destructive",
  copied: "border-emerald-500/20 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  skipped: "border-border bg-muted/60 text-muted-foreground",
};

const isActionableCopyItem = (item: EnvironmentFileCopyItemDto) =>
  item.status === "create" || item.status === "overwrite";

const canEditCopyTarget = (item: EnvironmentFileCopyItemDto) =>
  item.targetExists ||
  item.status === "copied" ||
  item.status === "skipped" ||
  item.status === "skipExists";

export const ProjectEnvironmentFileCopyDialog = ({
  open,
  projectId,
  targetEnvironmentSlug,
  targetLabel,
  sourceEnvironmentSlug = "primary",
  onOpenChange,
}: ProjectEnvironmentFileCopyDialogProps) => {
  const queryClient = useQueryClient();
  const [overwrite, setOverwrite] = useState(false);
  const [copyResultItems, setCopyResultItems] = useState<EnvironmentFileCopyItemDto[] | null>(null);
  const [selectedPaths, setSelectedPaths] = useState<string[]>([]);
  const [editorTarget, setEditorTarget] = useState<EnvironmentFileRequest | null>(null);
  const copyEnvironmentFiles = useMutation(projectMutations.copyEnvironmentFiles(queryClient));
  const request = useMemo(
    () =>
      ({
        projectId,
        sourceEnvironmentSlug,
        targetEnvironmentSlug,
        relativePaths: [],
        overwrite,
      }) satisfies EnvironmentFileCopyRequest,
    [overwrite, projectId, sourceEnvironmentSlug, targetEnvironmentSlug],
  );
  const previewQuery = projectQueries.environmentFileCopyPreview(request);
  const preview = useQuery({
    ...previewQuery,
    enabled: open && projectId.length > 0 && targetEnvironmentSlug.length > 0,
    refetchOnWindowFocus: false,
  });
  const previewItems = preview.data?.items;
  const items = useMemo(
    () => copyResultItems ?? previewItems ?? [],
    [copyResultItems, previewItems],
  );
  const actionablePaths = useMemo(
    () => items.filter(isActionableCopyItem).map((item) => item.relativePath),
    [items],
  );
  const actionablePathSet = useMemo(() => new Set(actionablePaths), [actionablePaths]);
  const selectedActionablePaths = selectedPaths.filter((path) => actionablePathSet.has(path));
  const actionableCount = actionablePaths.length;
  const importedCount = items.filter((item) => item.status === "copied").length;
  const blockedCount = items.filter((item) => item.status === "blocked").length;
  const allActionableSelected =
    actionableCount > 0 && selectedActionablePaths.length === actionableCount;
  const displayTarget = targetLabel ?? targetEnvironmentSlug;
  const importButtonLabel =
    selectedActionablePaths.length === 1
      ? "Import 1 file"
      : `Import ${selectedActionablePaths.length} files`;
  const footerSummary = copyResultItems
    ? `${importedCount} imported`
    : selectedActionablePaths.length > 0
      ? `${selectedActionablePaths.length} selected`
      : actionableCount > 0
        ? `${actionableCount} ready to import`
        : blockedCount > 0
          ? `${blockedCount} blocked`
          : "No files to import";

  useEffect(() => {
    if (!open || copyResultItems) {
      return;
    }

    setSelectedPaths(actionablePaths);
  }, [actionablePaths, copyResultItems, open]);

  const handleToggleAll = () => {
    setSelectedPaths(allActionableSelected ? [] : actionablePaths);
  };

  const handleTogglePath = (relativePath: string, selected: boolean) => {
    if (!actionablePathSet.has(relativePath)) {
      return;
    }

    setSelectedPaths((currentPaths) => {
      if (!selected) {
        return currentPaths.filter((path) => path !== relativePath);
      }

      if (currentPaths.includes(relativePath)) {
        return currentPaths;
      }

      return [...currentPaths, relativePath];
    });
  };

  const handleEditFile = (relativePath: string) => {
    setEditorTarget({
      projectId,
      environmentSlug: targetEnvironmentSlug,
      serviceId: null,
      relativePath,
    });
  };

  const handleCopy = async (relativePaths: string[]) => {
    if (relativePaths.length === 0) {
      return;
    }

    try {
      const result = await copyEnvironmentFiles.mutateAsync({
        ...request,
        relativePaths,
      });
      setCopyResultItems((currentItems) => {
        const baseItems = currentItems ?? preview.data?.items ?? [];
        const copiedByPath = new Map(
          result.items.map((item) => [item.relativePath, item] as const),
        );
        return baseItems.map((item) => copiedByPath.get(item.relativePath) ?? item);
      });
      setSelectedPaths((currentPaths) =>
        currentPaths.filter((path) => !relativePaths.includes(path)),
      );
      toast.success("Environment files imported", {
        description:
          relativePaths.length === 1 ? relativePaths[0] : `${relativePaths.length} files`,
      });
    } catch (error) {
      toast.error("Copy .env failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const handleOpenChange = (nextOpen: boolean) => {
    if (!nextOpen) {
      setCopyResultItems(null);
      setSelectedPaths([]);
      setEditorTarget(null);
      setOverwrite(false);
    }
    onOpenChange(nextOpen);
  };

  return (
    <>
      <Dialog open={open} onOpenChange={handleOpenChange}>
        <DialogContent className="grid max-h-[min(720px,calc(100vh-4rem))] grid-rows-[auto_minmax(0,1fr)_auto] gap-0 overflow-hidden sm:max-w-2xl">
          <DialogHeader className="p-5 pb-3">
            <DialogTitle>Import environment files</DialogTitle>
            <DialogDescription>
              Import .env files from {sourceEnvironmentSlug} into {displayTarget}.
            </DialogDescription>
          </DialogHeader>

          <DialogBody className="min-h-0 overflow-hidden px-5 pb-4">
            <div className="flex h-full min-h-0 flex-col gap-3">
              <div className="flex flex-col gap-3 rounded-lg border border-border/60 bg-muted/20 px-3 py-2.5 sm:flex-row sm:items-center sm:justify-between">
                <div className="flex min-w-0 items-center gap-2 text-xs">
                  <span className="truncate rounded-md bg-background px-2 py-1 font-mono text-foreground ring-1 ring-border/60">
                    {sourceEnvironmentSlug}
                  </span>
                  <ArrowRightIcon className="size-3.5 shrink-0 text-muted-foreground" />
                  <span className="truncate rounded-md bg-background px-2 py-1 font-mono text-foreground ring-1 ring-border/60">
                    {displayTarget}
                  </span>
                </div>
                <label className="flex shrink-0 items-center justify-between gap-3 text-xs text-muted-foreground sm:justify-end">
                  <span>Overwrite existing</span>
                  <Switch
                    checked={overwrite}
                    disabled={copyEnvironmentFiles.isPending}
                    onCheckedChange={(checked) => {
                      setCopyResultItems(null);
                      setSelectedPaths([]);
                      setOverwrite(checked);
                    }}
                  />
                </label>
              </div>

              {preview.isPending ? (
                <div className="space-y-2">
                  <Skeleton className="h-12 w-full" />
                  <Skeleton className="h-12 w-full" />
                  <Skeleton className="h-12 w-3/4" />
                </div>
              ) : preview.error ? (
                <div className="rounded-lg border border-destructive/20 bg-destructive/5 p-4 text-sm text-destructive">
                  {preview.error.message}
                </div>
              ) : items.length === 0 ? (
                <div className="rounded-lg border border-dashed border-border p-6 text-center">
                  <FileTextIcon className="mx-auto size-5 text-muted-foreground" />
                  <p className="mt-2 text-sm font-medium">No source environment files</p>
                  <p className="mt-1 text-xs text-muted-foreground">
                    Create or open an environment file in the primary environment first.
                  </p>
                </div>
              ) : (
                <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border border-border/60">
                  <div className="flex h-10 items-center gap-3 border-b border-border/60 bg-muted/20 px-3">
                    <input
                      type="checkbox"
                      className="size-4 shrink-0 accent-primary"
                      checked={allActionableSelected}
                      disabled={actionableCount === 0 || copyEnvironmentFiles.isPending}
                      aria-label="Select all importable environment files"
                      onChange={handleToggleAll}
                    />
                    <p className="min-w-0 flex-1 text-xs font-medium text-muted-foreground">
                      Environment files
                    </p>
                    <span className="shrink-0 text-xs text-muted-foreground">
                      {selectedActionablePaths.length}/{actionableCount} selected
                    </span>
                  </div>
                  <div className="min-h-0 flex-1 overflow-y-auto">
                    {items.map((item) => (
                      <EnvironmentFileCopyRow
                        key={item.relativePath}
                        item={item}
                        selected={selectedPaths.includes(item.relativePath)}
                        copying={copyEnvironmentFiles.isPending}
                        onSelectedChange={(selected) =>
                          handleTogglePath(item.relativePath, selected)
                        }
                        onEdit={() => handleEditFile(item.relativePath)}
                      />
                    ))}
                  </div>
                </div>
              )}
            </div>
          </DialogBody>

          <DialogFooter className="static border-t border-border/60 bg-popover px-5 py-3">
            <div className="flex w-full flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
              <p className="text-xs text-muted-foreground">{footerSummary}</p>
              <div className="flex justify-end gap-2">
                <Button type="button" variant="ghost" onClick={() => handleOpenChange(false)}>
                  Close
                </Button>
                <Button
                  type="button"
                  loading={copyEnvironmentFiles.isPending}
                  disabled={preview.isPending || selectedActionablePaths.length === 0}
                  onClick={() => void handleCopy(selectedActionablePaths)}
                >
                  <CopyIcon className="size-4" />
                  {selectedActionablePaths.length > 0 ? importButtonLabel : "Import files"}
                </Button>
              </div>
            </div>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ProjectEnvironmentFileDialog
        open={editorTarget !== null}
        target={editorTarget}
        onOpenChange={(nextOpen) => {
          if (!nextOpen) {
            setEditorTarget(null);
          }
        }}
      />
    </>
  );
};

interface EnvironmentFileCopyRowProps {
  item: EnvironmentFileCopyItemDto;
  selected: boolean;
  copying: boolean;
  onSelectedChange: (selected: boolean) => void;
  onEdit: () => void;
}

const EnvironmentFileCopyRow = ({
  item,
  selected,
  copying,
  onSelectedChange,
  onEdit,
}: EnvironmentFileCopyRowProps) => {
  const actionable = isActionableCopyItem(item);
  const editable = canEditCopyTarget(item);

  return (
    <div className="flex min-w-0 items-center gap-3 border-b border-border/60 px-3 py-2.5 last:border-b-0">
      <input
        type="checkbox"
        className="size-4 shrink-0 accent-primary disabled:opacity-40"
        checked={selected}
        disabled={!actionable || copying}
        aria-label={`Select ${item.relativePath}`}
        onChange={(event) => onSelectedChange(event.target.checked)}
      />
      <FileTextIcon className="size-4 shrink-0 text-muted-foreground" />
      <div className="min-w-0 flex-1">
        <p className="truncate font-mono text-xs text-foreground">{item.relativePath}</p>
        {item.message ? (
          <p className="mt-0.5 truncate text-xs text-muted-foreground">{item.message}</p>
        ) : null}
      </div>
      <span
        className={cn(
          "shrink-0 rounded-full border px-2 py-0.5 text-[10px] font-medium",
          statusClassName[item.status],
        )}
      >
        {statusLabel[item.status]}
      </span>
      {editable ? (
        <Button type="button" size="xs" variant="outline" disabled={copying} onClick={onEdit}>
          <FileTextIcon className="size-3" />
          Edit
        </Button>
      ) : null}
      {copying && actionable ? <SpinnerIcon className="size-3 shrink-0 animate-spin" /> : null}
    </div>
  );
};
