"use client";

import { useMutation, useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import type { EnvironmentFileDto, EnvironmentFileRequest } from "@/types/roxy";

import {
  CheckIcon,
  ExternalLinkIcon,
  FileTextIcon,
  SpinnerIcon,
  XIcon,
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
import { projectMutations, projectQueries } from "@/lib/queries";
import { cn } from "@/lib/utils";

import { EnvironmentFileCodeEditor } from "./environment-file-code-editor";

const emptyTarget = {
  projectId: "",
  environmentSlug: "",
  serviceId: null,
  relativePath: null,
} satisfies EnvironmentFileRequest;

function countLines(value: string): number {
  if (value.length === 0) return 1;

  let count = 1;
  for (const char of value) {
    if (char === "\n") count += 1;
  }

  return count;
}

interface ProjectEnvironmentFileDialogProps {
  open: boolean;
  target: EnvironmentFileRequest | null;
  onOpenChange: (open: boolean) => void;
}

export const ProjectEnvironmentFileDialog = ({
  open,
  target,
  onOpenChange,
}: ProjectEnvironmentFileDialogProps) => {
  const queryTarget = target ?? emptyTarget;
  const targetKey = `${queryTarget.projectId}\u0000${queryTarget.environmentSlug}\u0000${
    queryTarget.serviceId ?? ""
  }\u0000${queryTarget.relativePath ?? ""}`;
  const [readNonce, setReadNonce] = useState(0);
  const environmentFileQuery = projectQueries.environmentFile(queryTarget);
  const readEnvironmentFile = useQuery({
    ...environmentFileQuery,
    queryKey: [...environmentFileQuery.queryKey, readNonce] as const,
    enabled: open && target !== null && readNonce > 0,
    gcTime: 0,
    refetchOnWindowFocus: false,
  });
  const saveEnvironmentFile = useMutation(
    projectMutations.saveEnvironmentFile(),
  );
  const openEnvInEditor = useMutation(projectMutations.openEnvInEditor());
  const [loadedFile, setLoadedFile] = useState<EnvironmentFileDto | null>(null);
  const [draftContent, setDraftContent] = useState("");
  const [discardDialogOpen, setDiscardDialogOpen] = useState(false);
  const [saveInFlight, setSaveInFlight] = useState(false);
  const draftContentRef = useRef("");
  const loadedFileRef = useRef<EnvironmentFileDto | null>(null);
  const targetRef = useRef<EnvironmentFileRequest | null>(null);
  const dirtyRef = useRef(false);
  const savingRef = useRef(false);
  const dirty = loadedFile !== null && draftContent !== loadedFile.content;
  const canEdit =
    target !== null && loadedFile !== null && !readEnvironmentFile.isPending;
  const saving = saveInFlight || saveEnvironmentFile.isPending;
  const title = loadedFile?.fileName ?? ".env";
  const relativePath = loadedFile?.relativePath ?? "";
  const lineCount = loadedFile ? countLines(draftContent) : null;
  const status = saving
    ? "Saving"
    : dirty
      ? "Unsaved"
      : loadedFile && !loadedFile.exists
        ? "New"
        : loadedFile
          ? "Saved"
          : "Loading";

  useEffect(() => {
    targetRef.current = target;
    loadedFileRef.current = loadedFile;
    dirtyRef.current = dirty;
    savingRef.current = saving;
  }, [dirty, loadedFile, saving, target]);

  useEffect(() => {
    if (!open) {
      setLoadedFile(null);
      setDraftContent("");
      setDiscardDialogOpen(false);
      setSaveInFlight(false);
      draftContentRef.current = "";
      return;
    }

    setLoadedFile(null);
    setDraftContent("");
    setDiscardDialogOpen(false);
    setSaveInFlight(false);
    setReadNonce((nonce) => nonce + 1);
    draftContentRef.current = "";
  }, [open, targetKey]);

  useEffect(() => {
    if (!open || !readEnvironmentFile.data) return;

    setLoadedFile(readEnvironmentFile.data);
    setDraftContent(readEnvironmentFile.data.content);
    draftContentRef.current = readEnvironmentFile.data.content;
  }, [open, readEnvironmentFile.data]);

  const handleDraftChange = useCallback((value: string) => {
    draftContentRef.current = value;
    setDraftContent(value);
  }, []);

  const handleSave = useCallback(async () => {
    const currentTarget = targetRef.current;
    const currentLoadedFile = loadedFileRef.current;
    if (
      !currentTarget ||
      !currentLoadedFile ||
      !dirtyRef.current ||
      savingRef.current
    ) {
      return;
    }

    savingRef.current = true;
    setSaveInFlight(true);

    try {
      const saved = await saveEnvironmentFile.mutateAsync({
        ...currentTarget,
        content: draftContentRef.current,
        expectedModifiedAt: currentLoadedFile.modifiedAt ?? null,
      });
      loadedFileRef.current = saved;
      draftContentRef.current = saved.content;
      dirtyRef.current = false;
      setLoadedFile(saved);
      setDraftContent(saved.content);
      toast.success("Environment file saved", {
        description: saved.relativePath,
      });
    } catch (error) {
      toast.error("Save .env failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    } finally {
      savingRef.current = false;
      setSaveInFlight(false);
    }
  }, [saveEnvironmentFile]);

  useEffect(() => {
    if (!open) return;

    const handleKeyDown = (event: KeyboardEvent) => {
      const isSaveShortcut =
        (event.metaKey || event.ctrlKey) &&
        !event.altKey &&
        !event.shiftKey &&
        event.key.toLowerCase() === "s";

      if (!isSaveShortcut) return;

      event.preventDefault();
      event.stopPropagation();
      event.stopImmediatePropagation();

      if (!event.repeat) {
        void handleSave();
      }
    };

    document.addEventListener("keydown", handleKeyDown, { capture: true });

    return () => {
      document.removeEventListener("keydown", handleKeyDown, { capture: true });
    };
  }, [handleSave, open]);

  const handleOpenInEditor = async () => {
    if (!target || openEnvInEditor.isPending) {
      return;
    }

    try {
      await openEnvInEditor.mutateAsync(target);
    } catch (error) {
      toast.error("Open in editor failed", {
        description: error instanceof Error ? error.message : String(error),
      });
    }
  };

  const handleOpenChange = useCallback(
    (nextOpen: boolean) => {
      if (!nextOpen && dirtyRef.current) {
        setDiscardDialogOpen(true);
        return;
      }

      onOpenChange(nextOpen);
    },
    [onOpenChange],
  );

  const handleDiscardChanges = () => {
    setDiscardDialogOpen(false);
    dirtyRef.current = false;
    onOpenChange(false);
  };

  const handleCancelDiscard = () => {
    setDiscardDialogOpen(false);
  };

  const handleRetry = () => {
    setReadNonce((nonce) => nonce + 1);
  };

  const editor =
    readEnvironmentFile.isPending || readNonce === 0 || !target ? (
      <div className="flex h-full min-h-[360px] items-center justify-center rounded-[18px] bg-background/90 shadow-[inset_0_0_0_1px_rgba(0,0,0,0.06)] dark:shadow-[inset_0_0_0_1px_rgba(255,255,255,0.08)]">
        <SpinnerIcon className="size-4 animate-spin text-muted-foreground" />
      </div>
    ) : readEnvironmentFile.error ? (
      <div className="flex h-full min-h-[360px] flex-col items-center justify-center gap-3 rounded-[18px] bg-destructive/5 p-6 text-center shadow-[inset_0_0_0_1px_rgba(0,0,0,0.06)] dark:shadow-[inset_0_0_0_1px_rgba(255,255,255,0.08)]">
        <FileTextIcon className="size-5 text-destructive" />
        <p className="max-w-lg text-sm text-pretty text-destructive">
          {readEnvironmentFile.error.message}
        </p>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-10 transition-[scale,background-color,box-shadow,color] active:scale-[0.96]"
          onClick={handleRetry}
        >
          Retry
        </Button>
      </div>
    ) : (
      <div className="h-full min-h-[360px] rounded-3xl border bg-background/95 ">
        <EnvironmentFileCodeEditor
          value={draftContent}
          disabled={!canEdit || saving}
          onChange={handleDraftChange}
        />
      </div>
    );

  return (
    <>
      <Dialog open={open} onOpenChange={handleOpenChange}>
        <DialogContent
          showCloseButton={false}
          className="grid max-h-[min(860px,calc(100vh-2rem))] w-[min(1120px,calc(100vw-2rem))] grid-rows-[auto_minmax(0,1fr)_auto] gap-0 overflow-hidden rounded-[28px] bg-background/95 p-1 shadow-[0_0_0_1px_rgba(0,0,0,0.06),0_24px_80px_-36px_rgba(0,0,0,0.55),0_12px_32px_-22px_rgba(0,0,0,0.35)] backdrop-blur-2xl sm:max-w-none dark:bg-background/90 dark:shadow-[0_0_0_1px_rgba(255,255,255,0.09),0_24px_80px_-36px_rgba(0,0,0,0.85)]"
        >
          <DialogHeader className="gap-0 px-5 pt-4 pb-2">
            <div className="flex min-w-0 items-center justify-between gap-4">
              <div className="min-w-0">
                <DialogTitle className="min-w-0 truncate text-[15px] leading-5 font-semibold tracking-[-0.01em] text-balance">
                  Edit {title}
                </DialogTitle>
                <DialogDescription className="mt-0.5 flex min-w-0 items-center gap-2 text-pretty">
                  <span className="min-w-0 truncate font-mono text-[12px] text-muted-foreground">
                    {relativePath || "Loading"}
                  </span>
                  {lineCount ? (
                    <>
                      <span className="size-1 shrink-0 rounded-full bg-muted-foreground/35" />
                      <span className="shrink-0 font-mono text-[12px] text-muted-foreground tabular-nums">
                        {lineCount} {lineCount === 1 ? "line" : "lines"}
                      </span>
                    </>
                  ) : null}
                </DialogDescription>
              </div>

              <div className="flex shrink-0 items-center gap-2">
                <span
                  className={cn(
                    "inline-flex h-7 items-center gap-1.5 rounded-full px-2.5 text-xs font-medium transition-[background-color,color]",
                    dirty
                      ? "bg-primary/10 text-primary"
                      : loadedFile && !loadedFile.exists
                        ? "bg-muted/70 text-muted-foreground"
                        : "bg-transparent text-muted-foreground",
                  )}
                >
                  <span
                    className={cn(
                      "size-1.5 rounded-full",
                      dirty
                        ? "bg-primary"
                        : loadedFile
                          ? "bg-muted-foreground/60"
                          : "bg-muted-foreground/30",
                    )}
                  />
                  {status}
                </span>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="size-8 rounded-full text-muted-foreground transition-[scale,background-color,color] hover:text-foreground active:scale-[0.96]"
                  onClick={() => handleOpenChange(false)}
                >
                  <XIcon className="size-4" />
                  <span className="sr-only">Close</span>
                </Button>
              </div>
            </div>
          </DialogHeader>

          <DialogBody className="min-h-0 px-4 py-1">
            <div className=" ">
              <div className="flex flex-col  overflow-hidden h-[min(62vh,620px)] min-h-[380px] rounded-[24px] bg-background/95  dark:bg-background/75 dark:shadow-[0_0_0_1px_rgba(255,255,255,0.08)]">
                <div className="min-h-0 flex-1 ">{editor}</div>
              </div>
            </div>
          </DialogBody>

          <DialogFooter className="px-5 pt-3 pb-5">
            <Button
              type="button"
              variant="outline"
              loading={openEnvInEditor.isPending}
              className="h-10 rounded-xl px-3.5 transition-[scale,background-color,box-shadow,color] active:scale-[0.96]"
              onClick={() => void handleOpenInEditor()}
              disabled={!target || openEnvInEditor.isPending}
            >
              <ExternalLinkIcon />
              Open in editor
            </Button>
            <div className="flex min-w-0 justify-end gap-2 sm:ml-auto">
              <Button
                type="button"
                variant="outline"
                className="h-10 rounded-xl px-4 transition-[scale,background-color,box-shadow,color] active:scale-[0.96]"
                onClick={() => handleOpenChange(false)}
              >
                Close
              </Button>
              <Button
                type="button"
                loading={saving}
                className="h-10 rounded-xl px-4 transition-[scale,background-color,box-shadow,color] active:scale-[0.96]"
                onClick={() => void handleSave()}
                disabled={!dirty || !canEdit}
              >
                <CheckIcon />
                Save
              </Button>
            </div>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={discardDialogOpen} onOpenChange={setDiscardDialogOpen}>
        <DialogContent
          showCloseButton={false}
          className="gap-0 overflow-hidden rounded-[24px] p-0 shadow-[0_0_0_1px_rgba(0,0,0,0.06),0_24px_80px_-36px_rgba(0,0,0,0.55)] sm:max-w-sm"
        >
          <DialogHeader className="px-5 pt-5 pb-2">
            <DialogTitle className="text-base font-semibold text-balance">
              Discard unsaved changes?
            </DialogTitle>
            <DialogDescription className="text-pretty">
              Your edits to{" "}
              <span className="font-mono text-foreground">{title}</span> have
              not been saved.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="px-5 pt-3 pb-5">
            <Button
              type="button"
              variant="outline"
              className="h-10 rounded-xl px-4 transition-[scale,background-color,box-shadow,color] active:scale-[0.96]"
              onClick={handleCancelDiscard}
            >
              Keep editing
            </Button>
            <Button
              type="button"
              variant="danger"
              className="h-10 rounded-xl px-4 transition-[scale,background-color,box-shadow,color] active:scale-[0.96]"
              onClick={handleDiscardChanges}
            >
              Discard
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
};
