"use client";

import { RiDraggable, RiMoreLine } from "@remixicon/react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Reorder, useDragControls } from "motion/react";
import { useState, type PointerEvent } from "react";

import type { DomainInfoDto, ProjectInfoDto } from "@/types/roxy";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import { projectMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { ProjectDomainList } from "./projects-list-item-domains";

interface ProjectsListItemProps {
  project: ProjectInfoDto;
  domains: DomainInfoDto[];
  reorderEnabled?: boolean;
}

export const ProjectsListItem = ({
  project,
  domains,
  reorderEnabled = true,
}: ProjectsListItemProps) => {
  const queryClient = useQueryClient();
  const dragControls = useDragControls();
  const updateProject = useMutation(projectMutations.update(queryClient));
  const removeProject = useMutation(projectMutations.remove(queryClient));
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const containerClassName = cn(
    "group/card items- relative flex gap-3 rounded-xl border border-border/30 bg-background bg-neutral-50/50 px-3 py-3 transition-colors",
    "hover:border-border",
    !project.enabled && "opacity-60",
  );

  const handleHandlePointerDown = (event: PointerEvent) => {
    if (!reorderEnabled) {
      return;
    }

    event.preventDefault();
    dragControls.start(event);
  };

  const toggleEnabled = () => {
    updateProject.mutate({ id: project.id, enabled: !project.enabled });
  };

  const handleDeleteProject = async () => {
    setDeleteError(null);

    try {
      await removeProject.mutateAsync({
        id: project.id,
        deleteDomains: true,
      });
      setDeleteDialogOpen(false);
    } catch (error) {
      setDeleteError(error instanceof Error ? error.message : String(error));
    }
  };

  const body = (
    <>
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex items-center gap-2">
          <span className="truncate text-lg font-medium">{project.name}</span>
        </div>
        <ProjectDomainList project={project} domains={domains} />
      </div>

      <DropdownMenu>
        <DropdownMenuTrigger
          aria-label="Project actions"
          render={
            <Button size="icon" variant="ghost">
              <RiMoreLine className="size-4" />
            </Button>
          }
        />
        <DropdownMenuContent align="end">
          <DropdownMenuItem
            onClick={toggleEnabled}
            disabled={updateProject.isPending || removeProject.isPending}
          >
            {project.enabled ? "Disable" : "Enable"}
          </DropdownMenuItem>
          <DropdownMenuItem
            variant="destructive"
            onClick={() => {
              setDeleteError(null);
              setDeleteDialogOpen(true);
            }}
            disabled={removeProject.isPending}
          >
            Delete project
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  );

  return (
    <>
      {reorderEnabled ? (
        <Reorder.Item
          value={project.id}
          dragListener={false}
          dragControls={dragControls}
          className={containerClassName}
        >
          <Button
            aria-label="Drag to reorder"
            onPointerDown={handleHandlePointerDown}
            size="icon"
            variant="ghost"
            className="cursor-grab hover:bg-muted hover:text-foreground active:cursor-grabbing"
          >
            <RiDraggable className="size-4" />
          </Button>
          {body}
        </Reorder.Item>
      ) : (
        <div className={containerClassName}>
          <div className="size-9 shrink-0" />
          {body}
        </div>
      )}

      <Dialog
        open={deleteDialogOpen}
        onOpenChange={(nextOpen) => {
          setDeleteDialogOpen(nextOpen);
          if (!nextOpen) {
            setDeleteError(null);
          }
        }}
      >
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>Delete {project.name}?</DialogTitle>
            <DialogDescription>
              This permanently removes the project and unregisters every attached domain.
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-3">
            <div className="rounded-lg border border-destructive/20 bg-destructive/5 p-3">
              <p className="text-sm text-foreground">
                {domains.length === 0
                  ? "No domains are attached. Only the project will be removed."
                  : `This will remove ${domains.length} attached domain${
                      domains.length === 1 ? "" : "s"
                    }:`}
              </p>
              {domains.length > 0 ? (
                <ul className="mt-3 space-y-1">
                  {domains.map((domain) => (
                    <li
                      key={domain.registration.pattern}
                      className="font-mono text-xs text-muted-foreground"
                    >
                      {domain.registration.pattern}
                    </li>
                  ))}
                </ul>
              ) : null}
            </div>

            {deleteError ? <p className="text-sm text-destructive">{deleteError}</p> : null}
          </div>

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => setDeleteDialogOpen(false)}
              disabled={removeProject.isPending}
            >
              Cancel
            </Button>
            <Button
              type="button"
              variant="danger"
              loading={removeProject.isPending}
              onClick={handleDeleteProject}
            >
              Delete project
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
};

export const ProjectListItemSkeleton = () => {
  return (
    <div className="flex items-center gap-3 rounded-xl border border-border bg-background px-3 py-3">
      <div className="size-7 shrink-0" />
      <div className="flex flex-1 flex-col gap-1.5">
        <Skeleton className="h-4 w-32" />
        <div className="flex gap-1.5">
          <Skeleton className="h-6 w-24 rounded-full" />
          <Skeleton className="h-6 w-28 rounded-full" />
        </div>
      </div>
    </div>
  );
};
