"use client";

import { CheckIcon, CopyIcon } from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";

export interface RevealedPassword {
  url: string;
  password: string;
}

interface PublicUrlPasswordDialogProps {
  revealed: RevealedPassword | null;
  onClose: () => void;
}

/** Shows a generated password once: Localify keeps only its hash. */
export const PublicUrlPasswordDialog = ({ revealed, onClose }: PublicUrlPasswordDialogProps) => {
  const { copy, copied } = useCopyToClipboard();

  return (
    <Dialog
      open={revealed !== null}
      disablePointerDismissal
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent className="sm:max-w-md" showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Save the password</DialogTitle>
          <DialogDescription>
            Share it with the people who should open{" "}
            <span className="font-mono text-foreground">{revealed?.url}</span>. It cannot be shown
            again.
          </DialogDescription>
        </DialogHeader>
        <DialogBody>
          <div className="flex items-center gap-2 rounded-lg bg-muted/40 p-2 ring-1 ring-border/60">
            <code className="min-w-0 flex-1 truncate px-1 font-mono text-base font-semibold text-foreground select-all">
              {revealed?.password}
            </code>
            <Button
              type="button"
              variant="secondary"
              size="sm"
              onClick={() => {
                if (revealed) void copy(revealed.password);
              }}
            >
              {copied ? <CheckIcon className="text-emerald-500" /> : <CopyIcon />}
              {copied ? "Copied" : "Copy"}
            </Button>
          </div>
        </DialogBody>
        <DialogFooter>
          <DialogClose render={<Button className="sm:ml-auto" />}>Done</DialogClose>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
