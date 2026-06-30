"use client";

import { RiMoreLine } from "@remixicon/react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { toast } from "sonner";

import type {
  EnvironmentFileRequest,
  ProjectEnvironmentInfoDto,
  TerminalTargetRequest,
} from "@/types/roxy";

import { CopyIcon, FileTextIcon, FolderIcon, TerminalIcon, TrashIcon } from "@/components/icons";
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
import { projectMutations } from "@/lib/queries";
import { useTerminal } from "@/providers/terminal-provider";

import { ProjectEnvironmentFileCopyDialog } from "./project-environment-file-copy-dialog";
import { ProjectEnvironmentFileDialog } from "./project-environment-file-dialog";
import { ProjectEnvironmentFileMenuItem } from "./project-environment-file-menu-item";

interface ProjectsListItemEnvironmentActionsProps {
  environment: ProjectEnvironmentInfoDto;
  terminalTarget: TerminalTargetRequest | null;
}

export const ProjectsListItemEnvironmentActions = ({
  environment,
  terminalTarget,
}: ProjectsListItemEnvironmentActionsProps) => {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { openTerminal, setSelectedTarget } = useTerminal();
  const openInEditor = useMutation(projectMutations.openInEditor());
  const removeEnvironment = useMutation(projectMutations.removeEnvironment(queryClient));
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [environmentFileDialogOpen, setEnvironmentFileDialogOpen] = useState(false);
  const [environmentFileTarget, setEnvironmentFileTarget] = useState<EnvironmentFileRequest | null>(
    null,
  );
  const [copyEnvironmentFileDialogOpen, setCopyEnvironmentFileDialogOpen] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const handleOpenInEditor = async () => {
    if (!environment.rootPath) {
      return;
    }

    try {
      await openInEditor.mutateAsync({ path: environment.rootPath });
    } catch (error) {
      toast.error("Open in editor failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const handleOpenTerminal = async () => {
    if (!terminalTarget) {
      return;
    }

    setSelectedTarget(terminalTarget);

    try {
      await openTerminal(terminalTarget);
    } catch (error) {
      toast.error("Open terminal failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const handleViewLogs = () => {
    void navigate({
      to: "/logs",
      search: { projectId: environment.projectId, view: "requests" },
    });
  };

  const handleDeleteEnvironment = async () => {
    setDeleteError(null);

    try {
      await removeEnvironment.mutateAsync({
        projectId: environment.projectId,
        environmentSlug: environment.slug,
      });
      setDeleteDialogOpen(false);
      toast.success("Environment deleted", {
        description: environment.slug,
      });
    } catch (error) {
      setDeleteError(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <>
      <DropdownMenu open={menuOpen} onOpenChange={setMenuOpen}>
        <DropdownMenuTrigger
          aria-label={`Environment actions for ${environment.slug}`}
          render={
            <Button
              type="button"
              size="icon-xs"
              variant="ghost"
              className="text-muted-foreground hover:text-foreground"
            >
              <RiMoreLine className="size-3.5" />
            </Button>
          }
        />
        <DropdownMenuContent align="end" className="w-48">
          <DropdownMenuItem
            onClick={() => void handleOpenInEditor()}
            disabled={
              !environment.rootPath || openInEditor.isPending || removeEnvironment.isPending
            }
          >
            <FolderIcon className="size-4" />
            Open in editor
          </DropdownMenuItem>
          <ProjectEnvironmentFileMenuItem
            active={menuOpen}
            label="Open .env"
            target={{
              projectId: environment.projectId,
              environmentSlug: environment.slug,
              serviceId: null,
            }}
            disabled={!environment.rootPath || removeEnvironment.isPending}
            onOpenTarget={(nextTarget) => {
              setEnvironmentFileTarget(nextTarget);
              setEnvironmentFileDialogOpen(true);
            }}
          />
          {!environment.primary ? (
            <DropdownMenuItem
              onClick={() => setCopyEnvironmentFileDialogOpen(true)}
              disabled={!environment.rootPath || removeEnvironment.isPending}
            >
              <CopyIcon className="size-4" />
              Copy .env from primary
            </DropdownMenuItem>
          ) : null}
          <DropdownMenuItem
            onClick={() => void handleOpenTerminal()}
            disabled={!terminalTarget || removeEnvironment.isPending}
          >
            <TerminalIcon className="size-4" />
            Open terminal
          </DropdownMenuItem>
          <DropdownMenuItem onClick={handleViewLogs} disabled={removeEnvironment.isPending}>
            <FileTextIcon className="size-4" />
            View logs
          </DropdownMenuItem>
          <DropdownMenuItem
            variant="destructive"
            onClick={() => {
              setDeleteError(null);
              setDeleteDialogOpen(true);
            }}
            disabled={environment.primary || removeEnvironment.isPending}
          >
            <TrashIcon className="size-4" />
            Delete environment
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

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
            <DialogTitle>Delete {environment.slug}?</DialogTitle>
            <DialogDescription>
              This removes the environment and unregisters its attached domains.
            </DialogDescription>
          </DialogHeader>

          <DialogBody className="space-y-3">
            <div className="rounded-lg border border-destructive/20 bg-destructive/5 p-3">
              <p className="text-sm text-foreground">
                {environment.services.length === 0
                  ? "No domains are attached. Only the environment will be removed."
                  : `This will remove ${environment.services.length} attached domain${
                      environment.services.length === 1 ? "" : "s"
                    }:`}
              </p>
              {environment.services.length > 0 ? (
                <ul className="mt-3 space-y-1">
                  {environment.services.map((service) => (
                    <li key={service.domain} className="font-mono text-xs text-muted-foreground">
                      {service.domain}
                    </li>
                  ))}
                </ul>
              ) : null}
            </div>

            {deleteError ? <p className="text-sm text-destructive">{deleteError}</p> : null}
          </DialogBody>

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => setDeleteDialogOpen(false)}
              disabled={removeEnvironment.isPending}
            >
              Cancel
            </Button>
            <Button
              type="button"
              variant="danger"
              loading={removeEnvironment.isPending}
              onClick={handleDeleteEnvironment}
            >
              Delete environment
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ProjectEnvironmentFileDialog
        open={environmentFileDialogOpen}
        target={environmentFileTarget}
        onOpenChange={setEnvironmentFileDialogOpen}
      />
      <ProjectEnvironmentFileCopyDialog
        open={copyEnvironmentFileDialogOpen}
        projectId={environment.projectId}
        targetEnvironmentSlug={environment.slug}
        targetLabel={environment.slug}
        onOpenChange={setCopyEnvironmentFileDialogOpen}
      />
    </>
  );
};
