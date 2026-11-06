"use client";

import { revalidateLogic } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { toast } from "sonner";

import type { CloudAccountDto, PublicUrlDto } from "@/types/localify";

import { CheckIcon, CopyIcon, ExternalLinkIcon, LockIcon, LockOpenIcon } from "@/components/icons";
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
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useAppForm } from "@/hooks/form/form";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { bootstrapQueries, cloudMutations } from "@/lib/queries";
import { tauriClient } from "@/lib/tauri-client";
import {
  cloudStatusDotClassName,
  describePublicUrlState,
  wildcardHostPattern,
} from "@/lib/util.cloud";
import { cn } from "@/lib/utils";
import {
  type PublicUrlPasswordFormInput,
  optionalText,
  publicUrlPasswordFormSchema,
} from "@/lib/validators/validator.cloud";

import type { RevealedPassword } from "./public-url-password-dialog";

interface PublicUrlDetailsProps {
  publicUrl: PublicUrlDto;
  account: CloudAccountDto | undefined;
  onPasswordRevealed: (revealed: RevealedPassword) => void;
}

export const PublicUrlDetails = ({
  publicUrl,
  account,
  onPasswordRevealed,
}: PublicUrlDetailsProps) => {
  const queryClient = useQueryClient();
  const { data: bootstrap } = useQuery(bootstrapQueries.read());
  const claim = useMutation(cloudMutations.claimPublicUrl(queryClient));
  const openToAnyone = useMutation(cloudMutations.setProtection(queryClient));
  const remove = useMutation(cloudMutations.removePublicUrl(queryClient));
  const { copy, copied } = useCopyToClipboard();
  const [confirming, setConfirming] = useState<"open" | "remove" | null>(null);
  const [passwordFormOpen, setPasswordFormOpen] = useState(false);
  const status = describePublicUrlState(publicUrl, account);
  const isProtected = publicUrl.protection.mode === "password";

  const closeConfirm = () => {
    setConfirming(null);
    openToAnyone.reset();
    remove.reset();
  };

  const handleOpen = async () => {
    try {
      await tauriClient.openProjectUrl({
        url: publicUrl.url,
        browser: bootstrap?.launcherPreferences.browser ?? "default",
      });
    } catch (error) {
      const description = error instanceof Error ? error.message : "Unable to open this URL.";
      toast.error("Open in browser failed", { description });
    }
  };

  const handleClaim = async () => {
    try {
      await claim.mutateAsync({ id: publicUrl.id });
      toast.success("This Mac now serves the public URL");
    } catch (error) {
      const description = error instanceof Error ? error.message : String(error);
      toast.error("Could not serve it from this Mac", { description });
    }
  };

  const handleOpenToAnyone = async () => {
    try {
      await openToAnyone.mutateAsync({ id: publicUrl.id, mode: "public" });
      closeConfirm();
      toast.success("Password turned off");
    } catch {
      // The confirmation dialog shows the error.
    }
  };

  const handleRemove = async () => {
    try {
      await remove.mutateAsync({ id: publicUrl.id });
      closeConfirm();
      toast.success("Public URL removed");
    } catch {
      // The confirmation dialog shows the error.
    }
  };

  return (
    <>
      {/* min-w-0: as a grid item it would otherwise grow to fit a long URL. */}
      <DialogBody className="min-w-0 space-y-5">
        <div className="space-y-2 rounded-xl border border-border/60 bg-background/70 p-3">
          <div className="flex items-center gap-1">
            <span className="min-w-0 flex-1 truncate px-1 font-mono text-sm text-foreground select-all">
              {publicUrl.url}
            </span>
            <Tooltip>
              <TooltipTrigger
                render={
                  <Button
                    type="button"
                    size="icon-sm"
                    variant="ghost"
                    aria-label={copied ? "URL copied" : "Copy URL"}
                    onClick={() => void copy(publicUrl.url)}
                  >
                    {copied ? <CheckIcon className="text-emerald-500" /> : <CopyIcon />}
                  </Button>
                }
              />
              <TooltipContent>{copied ? "URL copied" : "Copy URL"}</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger
                render={
                  <Button
                    type="button"
                    size="icon-sm"
                    variant="ghost"
                    aria-label="Open in browser"
                    onClick={() => void handleOpen()}
                  >
                    <ExternalLinkIcon />
                  </Button>
                }
              />
              <TooltipContent>Open in browser</TooltipContent>
            </Tooltip>
          </div>
          {publicUrl.wildcard ? (
            <p className="px-1 text-xs leading-normal text-muted-foreground">
              Environments open at{" "}
              <span className="font-mono text-foreground">{wildcardHostPattern(publicUrl)}</span>,
              for{" "}
              <span className="font-mono text-foreground">
                &lt;label&gt;.{publicUrl.localDomain}
              </span>
              .
            </p>
          ) : null}
        </div>

        <div className="flex items-center gap-3">
          {status.tone ? (
            <span
              aria-hidden
              className={cn("size-2 shrink-0 rounded-full", cloudStatusDotClassName[status.tone])}
            />
          ) : null}
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium text-foreground">{status.label}</div>
            {status.detail ? (
              <div className="text-xs leading-normal text-muted-foreground">{status.detail}</div>
            ) : null}
          </div>
          {!publicUrl.servedHere ? (
            <Button
              type="button"
              size="sm"
              variant="outline"
              loading={claim.isPending}
              onClick={() => void handleClaim()}
            >
              Serve from this Mac
            </Button>
          ) : null}
        </div>

        <section className="space-y-3 border-t border-border/40 pt-5">
          <div className="flex items-start gap-3">
            <span
              aria-hidden
              className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-foreground/80 ring-1 ring-border/60 [&>svg]:size-4"
            >
              {isProtected ? <LockIcon /> : <LockOpenIcon />}
            </span>
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium text-foreground">
                {isProtected ? "Password protected" : "Open to anyone with the link"}
              </div>
              <div className="text-xs leading-normal text-muted-foreground">
                {isProtected
                  ? "Each browser asks for the password, then remembers it for 7 days."
                  : "No password. Anyone who has the URL can open your dev server."}
              </div>
            </div>
            {passwordFormOpen ? null : isProtected ? (
              <div className="flex shrink-0 gap-1.5">
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  onClick={() => setPasswordFormOpen(true)}
                >
                  Change password
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  onClick={() => setConfirming("open")}
                >
                  Turn off
                </Button>
              </div>
            ) : (
              <Button
                type="button"
                size="sm"
                variant="secondary"
                onClick={() => setPasswordFormOpen(true)}
              >
                Turn on password
              </Button>
            )}
          </div>

          {passwordFormOpen ? (
            <PublicUrlPasswordForm
              publicUrl={publicUrl}
              isProtected={isProtected}
              onClose={() => setPasswordFormOpen(false)}
              onPasswordRevealed={onPasswordRevealed}
            />
          ) : null}
        </section>
      </DialogBody>

      <DialogFooter>
        <Button type="button" variant="danger" onClick={() => setConfirming("remove")}>
          Remove public URL
        </Button>
        <DialogClose render={<Button variant="outline" />}>Done</DialogClose>
      </DialogFooter>

      <ConfirmDialog
        open={confirming === "open"}
        title="Open it to anyone?"
        description={
          <>
            Anyone who has the link can open{" "}
            <span className="font-mono text-foreground">{publicUrl.localDomain}</span>, with no
            password.
          </>
        }
        confirmLabel="Open to anyone"
        pending={openToAnyone.isPending}
        error={openToAnyone.error?.message ?? null}
        onConfirm={() => void handleOpenToAnyone()}
        onClose={closeConfirm}
      />
      <ConfirmDialog
        open={confirming === "remove"}
        title="Remove public URL?"
        description={
          <>
            <span className="font-mono text-foreground">{publicUrl.url}</span> stops working for
            everyone. The local domain does not change.
          </>
        }
        confirmLabel="Remove public URL"
        pending={remove.isPending}
        error={remove.error?.message ?? null}
        onConfirm={() => void handleRemove()}
        onClose={closeConfirm}
      />
    </>
  );
};

interface PublicUrlPasswordFormProps {
  publicUrl: PublicUrlDto;
  isProtected: boolean;
  onClose: () => void;
  onPasswordRevealed: (revealed: RevealedPassword) => void;
}

/** Sets a new password: changing it signs every browser out. */
const PublicUrlPasswordForm = ({
  publicUrl,
  isProtected,
  onClose,
  onPasswordRevealed,
}: PublicUrlPasswordFormProps) => {
  const queryClient = useQueryClient();
  const setProtection = useMutation(cloudMutations.setProtection(queryClient));
  const [submitError, setSubmitError] = useState<string | null>(null);

  const form = useAppForm({
    defaultValues: { password: "" } as PublicUrlPasswordFormInput,
    validationLogic: revalidateLogic({ mode: "submit", modeAfterSubmission: "change" }),
    validators: { onDynamic: publicUrlPasswordFormSchema },
    onSubmit: async ({ value }) => {
      setSubmitError(null);
      try {
        const result = await setProtection.mutateAsync({
          id: publicUrl.id,
          mode: "password",
          password: optionalText(value.password),
        });
        onClose();
        if (result.password) {
          onPasswordRevealed({ url: result.publicUrl.url, password: result.password });
        } else {
          toast.success(isProtected ? "Password changed" : "Password turned on");
        }
      } catch (error) {
        setSubmitError(error instanceof Error ? error.message : String(error));
      }
    },
  });

  return (
    <form.AppForm>
      <form
        noValidate
        className="space-y-3 rounded-xl border border-border/60 bg-muted/30 p-3"
        onSubmit={(event) => {
          event.preventDefault();
          void form.handleSubmit();
        }}
      >
        <form.AppField name="password">
          {(field) => (
            <field.TextField
              label={isProtected ? "New password (optional)" : "Password (optional)"}
              placeholder="Leave empty to generate one"
              description="At least 8 characters."
              autoFocus
            />
          )}
        </form.AppField>
        {isProtected ? (
          <p className="text-xs leading-normal text-muted-foreground">
            Every browser that signed in with the old password has to sign in again.
          </p>
        ) : null}
        {submitError ? (
          <p role="alert" className="text-xs text-destructive">
            {submitError}
          </p>
        ) : null}
        <div className="flex justify-end gap-2">
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <form.SubmitButton pendingLabel="Saving…">
            {isProtected ? "Change password" : "Turn on password"}
          </form.SubmitButton>
        </div>
      </form>
    </form.AppForm>
  );
};

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  description: ReactNode;
  confirmLabel: string;
  pending: boolean;
  error: string | null;
  onConfirm: () => void;
  onClose: () => void;
}

const ConfirmDialog = ({
  open,
  title,
  description,
  confirmLabel,
  pending,
  error,
  onConfirm,
  onClose,
}: ConfirmDialogProps) => (
  <Dialog
    open={open}
    onOpenChange={(nextOpen) => {
      if (!nextOpen) onClose();
    }}
  >
    <DialogContent className="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>
      {error ? (
        <DialogBody>
          <p role="alert" className="text-xs text-destructive">
            {error}
          </p>
        </DialogBody>
      ) : null}
      <DialogFooter>
        <DialogClose render={<Button variant="outline" />} disabled={pending}>
          Cancel
        </DialogClose>
        <Button type="button" variant="danger" loading={pending} onClick={onConfirm}>
          {confirmLabel}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
);
