"use client";

import { RiDraggable, RiMoreLine } from "@remixicon/react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Reorder, useDragControls } from "motion/react";
import { useState, type MouseEvent, type PointerEvent } from "react";
import { toast } from "sonner";

import type {
  DomainInfoDto,
  EnvironmentFileRequest,
  ProjectEnvironmentInfoDto,
  ProjectInfoDto,
  ProjectServiceInfoDto,
  TerminalTargetRequest,
} from "@/types/roxy";

import { PinIcon, PinSolidIcon, TerminalIcon } from "@/components/icons";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Skeleton } from "@/components/ui/skeleton";
import { projectMutations } from "@/lib/queries";
import { cn } from "@/lib/utils";
import {
  terminalTargetRequestMatches,
  useTerminal,
} from "@/providers/terminal-provider";

import { ProjectEnvironmentFileDialog } from "./project-environment-file-dialog";
import { ProjectEnvironmentFileMenuItem } from "./project-environment-file-menu-item";
import { ProjectsListItemDevDx } from "./projects-list-item-dev-dx";
import { ProjectDomainList } from "./projects-list-item-domains";

interface ProjectsListItemProps {
  project: ProjectInfoDto;
  domains: DomainInfoDto[];
  services: ProjectServiceInfoDto[];
  environments: ProjectEnvironmentInfoDto[];
  isPinned?: boolean;
  onTogglePinned?: () => void;
  reorderEnabled?: boolean;
}

export const ProjectsListItem = ({
  project,
  domains,
  services,
  environments,
  isPinned = false,
  onTogglePinned,
  reorderEnabled = true,
}: ProjectsListItemProps) => {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { openTerminal, selectedTarget, setSelectedTarget } = useTerminal();
  const dragControls = useDragControls();
  const updateProject = useMutation(projectMutations.update(queryClient));
  const removeProject = useMutation(projectMutations.remove(queryClient));
  const openInEditor = useMutation(projectMutations.openInEditor());
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [environmentFileDialogOpen, setEnvironmentFileDialogOpen] =
    useState(false);
  const [environmentFileTarget, setEnvironmentFileTarget] =
    useState<EnvironmentFileRequest | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [editorError, setEditorError] = useState<string | null>(null);
  const environmentDomainPatterns = new Set(
    environments
      .filter((environment) => !environment.primary)
      .flatMap((environment) =>
        environment.services.map((service) => service.domain),
      ),
  );
  const primaryDomains = domains.filter(
    (domain) => !environmentDomainPatterns.has(domain.registration.pattern),
  );
  const primaryEnvironment =
    environments.find((environment) => environment.primary) ??
    environments[0] ??
    null;
  const projectTerminalTarget =
    project.rootPath && primaryEnvironment
      ? ({
          kind: "project",
          projectId: project.id,
          environmentSlug: primaryEnvironment.slug,
        } satisfies TerminalTargetRequest)
      : null;
  const selected = terminalTargetRequestMatches(
    selectedTarget,
    projectTerminalTarget,
  );

  const containerClassName = cn(
    "group/card items- relative flex gap-3 rounded-xl border border-border/30  bg-neutral-50/50 px-3 py-3 transition-colors dark:bg-neutral-800/50",
    "hover:border-border",
    selected && "border-foreground/20 bg-muted/60 ring-1 ring-foreground/10",
    isPinned && "bg-primary/[0.04] dark:bg-primary/[0.08]",
    !project.enabled && "opacity-60",
  );

  const selectProjectTerminalTarget = () => {
    if (projectTerminalTarget) {
      setSelectedTarget(projectTerminalTarget);
    }
  };

  const handleCardClick = (event: MouseEvent<HTMLElement>) => {
    const target = event.target instanceof HTMLElement ? event.target : null;
    if (
      target?.closest(
        "a,button,input,select,textarea,[role='button'],[role='menuitem']",
      )
    ) {
      return;
    }

    selectProjectTerminalTarget();
  };

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

  const handleViewLogs = () => {
    void navigate({
      to: "/logs",
      search: { projectId: project.id, view: "requests" },
    });
  };

  const handleOpenInEditor = async () => {
    if (!project.rootPath) {
      return;
    }

    setEditorError(null);

    try {
      await openInEditor.mutateAsync({ path: project.rootPath });
    } catch (error) {
      setEditorError(error instanceof Error ? error.message : String(error));
    }
  };

  const handleOpenTerminal = async () => {
    if (!projectTerminalTarget) {
      return;
    }

    setSelectedTarget(projectTerminalTarget);

    try {
      await openTerminal(projectTerminalTarget);
    } catch (error) {
      toast.error("Open terminal failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const body = (
    <>
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex items-center gap-2">
          <span className="truncate text-lg font-medium">{project.name}</span>
          {isPinned ? (
            <PinSolidIcon
              aria-label="Pinned project"
              className="size-4 shrink-0 text-muted-foreground"
            />
          ) : null}
        </div>
        <ProjectDomainList
          project={project}
          domains={primaryDomains}
          environments={environments}
          services={services}
          totalDomainCount={domains.length}
        />
        <ProjectsListItemDevDx
          domains={domains}
          services={services}
          environments={environments}
          projectRootPath={project.rootPath}
        />
        {editorError ? (
          <p className="text-xs text-destructive">{editorError}</p>
        ) : null}
      </div>

      <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
        <DropdownMenuTrigger
          aria-label="Project actions"
          render={
            <Button size="icon" variant="ghost">
              <RiMoreLine className="size-4" />
            </Button>
          }
        />
        <DropdownMenuContent align="end" className={"w-fit"}>
          {onTogglePinned ? (
            <DropdownMenuItem
              onClick={onTogglePinned}
              disabled={removeProject.isPending}
              className="text-nowrap"
            >
              {isPinned ? (
                <PinSolidIcon className="size-4" />
              ) : (
                <PinIcon className="size-4" />
              )}
              {isPinned ? "Unpin project" : "Pin project"}
            </DropdownMenuItem>
          ) : null}
          <DropdownMenuItem
            onClick={() => void handleOpenInEditor()}
            disabled={
              !project.rootPath ||
              openInEditor.isPending ||
              removeProject.isPending
            }
          >
            Open in editor
          </DropdownMenuItem>
          <ProjectEnvironmentFileMenuItem
            active={menuOpen}
            label="Open .env"
            target={
              primaryEnvironment
                ? {
                    projectId: project.id,
                    environmentSlug: primaryEnvironment.slug,
                    serviceId: null,
                  }
                : null
            }
            disabled={!project.rootPath || removeProject.isPending}
            onOpenTarget={(nextTarget) => {
              setEnvironmentFileTarget(nextTarget);
              setEnvironmentFileDialogOpen(true);
            }}
          />
          <DropdownMenuItem
            onClick={() => void handleOpenTerminal()}
            disabled={!projectTerminalTarget || removeProject.isPending}
          >
            <TerminalIcon className="size-4" />
            Open terminal
          </DropdownMenuItem>
          <DropdownMenuItem
            onClick={handleViewLogs}
            disabled={removeProject.isPending}
          >
            View logs
          </DropdownMenuItem>
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
          onClick={handleCardClick}
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
        <div className={containerClassName} onClick={handleCardClick}>
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
              This permanently removes the project and unregisters every
              attached domain.
            </DialogDescription>
          </DialogHeader>

          <DialogBody className="space-y-3">
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

            {deleteError ? (
              <p className="text-sm text-destructive">{deleteError}</p>
            ) : null}
          </DialogBody>

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

      <ProjectEnvironmentFileDialog
        open={environmentFileDialogOpen}
        target={environmentFileTarget}
        onOpenChange={setEnvironmentFileDialogOpen}
      />
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
