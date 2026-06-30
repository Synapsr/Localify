"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";

import type { ProjectInfoDto, ProjectWorktreeDto } from "@/types/roxy";

import {
  CheckIcon,
  CodeIcon,
  CopyIcon,
  FolderIcon,
  PlusIcon,
  RefreshIcon,
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
import { Skeleton } from "@/components/ui/skeleton";
import { projectMutations, projectQueries } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { ProjectEnvironmentFileCopyDialog } from "./project-environment-file-copy-dialog";

interface ProjectWorktreesDialogProps {
  project: ProjectInfoDto;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const statusCopy = {
  primary: {
    label: "Primary",
    className: "border-border bg-muted/60 text-muted-foreground",
  },
  attached: {
    label: "Attached",
    className: "border-emerald-500/20 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  },
  available: {
    label: "Ready",
    className: "border-primary/20 bg-primary/10 text-primary",
  },
  unavailable: {
    label: "Unavailable",
    className: "border-destructive/20 bg-destructive/10 text-destructive",
  },
} satisfies Record<ProjectWorktreeDto["status"], { label: string; className: string }>;

export const ProjectWorktreesDialog = ({
  project,
  open,
  onOpenChange,
}: ProjectWorktreesDialogProps) => {
  const queryClient = useQueryClient();
  const [copyTarget, setCopyTarget] = useState<ProjectWorktreeDto | null>(null);
  const worktreesQuery = projectQueries.worktrees({ projectId: project.id });
  const worktrees = useQuery({
    ...worktreesQuery,
    enabled: open,
    refetchOnWindowFocus: false,
  });
  const attachWorktree = useMutation(projectMutations.attachWorktree(queryClient));

  const handleAttach = async (worktree: ProjectWorktreeDto) => {
    try {
      const result = await attachWorktree.mutateAsync({
        projectId: project.id,
        rootPath: worktree.rootPath,
        primaryRootPath: worktrees.data?.primaryRootPath ?? null,
      });
      toast.success(result.created ? "Worktree attached" : "Worktree already attached", {
        description: result.environment.slug,
      });
      await worktrees.refetch();
    } catch (error) {
      toast.error("Attach worktree failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const handleOpenChange = (nextOpen: boolean) => {
    if (!nextOpen) {
      setCopyTarget(null);
    }
    onOpenChange(nextOpen);
  };

  return (
    <>
      <Dialog open={open} onOpenChange={handleOpenChange}>
        <DialogContent className="grid max-h-[min(720px,calc(100vh-4rem))] grid-rows-[auto_minmax(0,1fr)_auto] gap-0 overflow-hidden sm:max-w-3xl">
          <DialogHeader className="p-5 pb-3">
            <DialogTitle>Find worktrees</DialogTitle>
            <DialogDescription>
              Attach a Git worktree as an environment for {project.name}.
            </DialogDescription>
          </DialogHeader>

          <DialogBody className="min-h-0 px-5 pb-4">
            {worktrees.isPending ? (
              <div className="space-y-2">
                <Skeleton className="h-16 w-full" />
                <Skeleton className="h-16 w-full" />
                <Skeleton className="h-16 w-4/5" />
              </div>
            ) : worktrees.error ? (
              <div className="rounded-lg border border-destructive/20 bg-destructive/5 p-4 text-sm text-destructive">
                {worktrees.error.message}
              </div>
            ) : worktrees.data.worktrees.length === 0 ? (
              <div className="rounded-lg border border-dashed border-border p-8 text-center">
                <CodeIcon className="mx-auto size-5 text-muted-foreground" />
                <p className="mt-2 text-sm font-medium">No Git worktrees found</p>
                <p className="mt-1 text-xs text-muted-foreground">
                  Create one with Git or Codex, then refresh this list.
                </p>
              </div>
            ) : (
              <div className="max-h-[56vh] overflow-y-auto rounded-lg border border-border/60">
                {worktrees.data.worktrees.map((worktree) => (
                  <WorktreeRow
                    key={worktree.rootPath}
                    worktree={worktree}
                    attaching={attachWorktree.isPending}
                    onAttach={() => void handleAttach(worktree)}
                    onCopyEnv={() => setCopyTarget(worktree)}
                  />
                ))}
              </div>
            )}
          </DialogBody>

          <DialogFooter className="static border-t border-border/60 bg-popover px-5 py-3">
            <div className="ml-auto flex items-center gap-2">
              <Button type="button" variant="ghost" onClick={() => handleOpenChange(false)}>
                Close
              </Button>
              <Button
                type="button"
                variant="secondary"
                loading={worktrees.isFetching && !worktrees.isPending}
                onClick={() => void worktrees.refetch()}
              >
                <RefreshIcon className="size-4" />
                Refresh
              </Button>
            </div>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {copyTarget?.attachedEnvironmentSlug ? (
        <ProjectEnvironmentFileCopyDialog
          open={copyTarget !== null}
          projectId={project.id}
          targetEnvironmentSlug={copyTarget.attachedEnvironmentSlug}
          targetLabel={copyTarget.slug}
          onOpenChange={(nextOpen) => {
            if (!nextOpen) {
              setCopyTarget(null);
            }
          }}
        />
      ) : null}
    </>
  );
};

interface WorktreeRowProps {
  worktree: ProjectWorktreeDto;
  attaching: boolean;
  onAttach: () => void;
  onCopyEnv: () => void;
}

const WorktreeRow = ({ worktree, attaching, onAttach, onCopyEnv }: WorktreeRowProps) => {
  const status = statusCopy[worktree.status];
  const canCopyEnv = worktree.status === "attached" && worktree.attachedEnvironmentSlug !== null;
  const canAttach = worktree.status === "available";

  return (
    <div className="border-b border-border/60 last:border-b-0">
      <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-3 px-4 py-3">
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-2">
            <span
              className={cn(
                "rounded-full border px-2 py-0.5 text-[10px] font-medium",
                status.className,
              )}
            >
              {status.label}
            </span>
            <span className="truncate text-sm font-medium text-foreground">{worktree.slug}</span>
            {worktree.gitBranch ? (
              <span className="hidden truncate text-xs text-muted-foreground sm:inline">
                {worktree.gitBranch}
              </span>
            ) : null}
          </div>
          <div className="mt-1 flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
            <FolderIcon className="size-3.5 shrink-0" />
            <span className="truncate font-mono">{worktree.rootPath}</span>
          </div>
          {worktree.unavailableReason ? (
            <p className="mt-1 text-xs text-destructive">{worktree.unavailableReason}</p>
          ) : worktree.serviceCandidates.length > 0 ? (
            <p className="mt-1 text-xs text-muted-foreground">
              {worktree.serviceCandidates.length} new service
              {worktree.serviceCandidates.length === 1 ? "" : "s"} detected in this worktree
            </p>
          ) : null}
        </div>

        <div className="flex shrink-0 items-center gap-1.5">
          {canCopyEnv ? (
            <Button type="button" size="xs" variant="outline" onClick={onCopyEnv}>
              <CopyIcon className="size-3" />
              Copy .env
            </Button>
          ) : null}
          {canAttach ? (
            <Button type="button" size="xs" loading={attaching} onClick={onAttach}>
              <PlusIcon className="size-3" />
              Add
            </Button>
          ) : worktree.status === "attached" ? (
            <CheckIcon className="size-4 text-emerald-600" />
          ) : null}
        </div>
      </div>
    </div>
  );
};
