"use client";

import { useQuery } from "@tanstack/react-query";

import type { EnvironmentFileCandidateDto, EnvironmentFileRequest } from "@/types/roxy";

import { FileTextIcon, SpinnerIcon } from "@/components/icons";
import {
  DropdownMenuItem,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
} from "@/components/ui/dropdown-menu";
import { projectQueries } from "@/lib/queries";

const emptyTarget = {
  projectId: "",
  environmentSlug: "",
  serviceId: null,
} satisfies EnvironmentFileRequest;

interface ProjectEnvironmentFileMenuItemProps {
  target: EnvironmentFileRequest | null;
  label: string;
  active: boolean;
  disabled?: boolean;
  onOpenTarget: (target: EnvironmentFileRequest) => void;
}

export const ProjectEnvironmentFileMenuItem = ({
  target,
  label,
  active,
  disabled = false,
  onOpenTarget,
}: ProjectEnvironmentFileMenuItemProps) => {
  const queryTarget = target
    ? ({
        projectId: target.projectId,
        environmentSlug: target.environmentSlug,
        serviceId: target.serviceId ?? null,
      } satisfies EnvironmentFileRequest)
    : emptyTarget;
  const environmentFilesQuery = projectQueries.environmentFiles(queryTarget);
  const environmentFiles = useQuery({
    ...environmentFilesQuery,
    enabled: active && target !== null && !disabled,
    refetchOnWindowFocus: false,
  });
  const files = environmentFiles.data?.files ?? [];

  const handleOpen = (file: EnvironmentFileCandidateDto | null) => {
    if (!target) return;

    onOpenTarget({
      ...target,
      relativePath: file?.relativePath ?? target.relativePath ?? null,
    });
  };

  const menuLabel = (
    <>
      <FileTextIcon className="size-4" />
      {label}
    </>
  );

  if (!target || disabled) {
    return <DropdownMenuItem disabled>{menuLabel}</DropdownMenuItem>;
  }

  if (active && environmentFiles.isPending) {
    return (
      <DropdownMenuItem disabled>
        <SpinnerIcon className="size-4 animate-spin" />
        Loading .env files
      </DropdownMenuItem>
    );
  }

  if (active && environmentFiles.error) {
    return (
      <DropdownMenuItem disabled>
        <FileTextIcon className="size-4" />
        Cannot load .env files
      </DropdownMenuItem>
    );
  }

  if (files.length <= 1) {
    return (
      <DropdownMenuItem
        onClick={(event) => {
          event.stopPropagation();
          handleOpen(files[0] ?? null);
        }}
      >
        {menuLabel}
      </DropdownMenuItem>
    );
  }

  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>{menuLabel}</DropdownMenuSubTrigger>
      <DropdownMenuSubContent className="w-72">
        {files.map((file) => (
          <DropdownMenuItem
            key={file.relativePath}
            onClick={(event) => {
              event.stopPropagation();
              handleOpen(file);
            }}
            className="items-start gap-2"
          >
            <FileTextIcon className="mt-0.5 size-4" />
            <span className="flex min-w-0 flex-1 flex-col">
              <span className="truncate text-sm">
                {file.rootLabel === "Project root"
                  ? file.fileName
                  : `${file.rootLabel}/${file.fileName}`}
              </span>
              <span className="truncate font-mono text-xs text-muted-foreground">
                {file.relativePath}
              </span>
            </span>
            {!file.exists ? (
              <span className="mt-0.5 shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">
                New
              </span>
            ) : null}
          </DropdownMenuItem>
        ))}
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  );
};
