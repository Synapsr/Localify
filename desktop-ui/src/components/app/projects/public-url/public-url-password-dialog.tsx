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
  /** The URL with the password, which signs a browser in. */
  passwordLink: string | null;
}

interface PublicUrlPasswordDialogProps {
  revealed: RevealedPassword | null;
  onClose: () => void;
}

/** Shows a generated password once: Localify Cloud keeps only its hash. */
export const PublicUrlPasswordDialog = ({ revealed, onClose }: PublicUrlPasswordDialogProps) => (
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
          <span className="font-mono text-foreground">{revealed?.url}</span>, or share the link that
          carries it. The password cannot be shown again.
        </DialogDescription>
      </DialogHeader>
      <DialogBody className="min-w-0 space-y-3">
        <CopyRow label="Password" value={revealed?.password ?? ""} emphasized />
        {revealed?.passwordLink ? (
          <CopyRow label="Link with password" value={revealed.passwordLink} />
        ) : null}
      </DialogBody>
      <DialogFooter>
        <DialogClose render={<Button className="sm:ml-auto" />}>Done</DialogClose>
      </DialogFooter>
    </DialogContent>
  </Dialog>
);

interface CopyRowProps {
  label: string;
  value: string;
  emphasized?: boolean;
}

const CopyRow = ({ label, value, emphasized = false }: CopyRowProps) => {
  const { copy, copied } = useCopyToClipboard();

  return (
    <div className="space-y-1.5">
      <div className="text-xs font-medium text-muted-foreground">{label}</div>
      <div className="flex items-center gap-2 rounded-lg bg-muted/40 p-2 ring-1 ring-border/60">
        <code
          className={
            emphasized
              ? "min-w-0 flex-1 truncate px-1 font-mono text-base font-semibold text-foreground select-all"
              : "min-w-0 flex-1 truncate px-1 font-mono text-sm text-foreground select-all"
          }
        >
          {value}
        </code>
        <Button
          type="button"
          variant="secondary"
          size="sm"
          aria-label={`Copy ${label.toLowerCase()}`}
          onClick={() => void copy(value)}
        >
          {copied ? <CheckIcon className="text-emerald-500" /> : <CopyIcon />}
          {copied ? "Copied" : "Copy"}
        </Button>
      </div>
    </div>
  );
};
