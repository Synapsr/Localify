"use client";

import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { type ReactNode, useState } from "react";
import { toast } from "sonner";

import type { PublicUrlWithPasswordDto } from "@/types/localify";

import { SpinnerIcon } from "@/components/icons";
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
import { bootstrapQueries, cloudQueries } from "@/lib/queries";
import {
  findPublicUrl,
  linkOnHost,
  projectNamespaceDomain,
  publicUrlOf,
  publicUrlTarget,
  SETTINGS_CLOUD_ANCHOR,
} from "@/lib/util.cloud";

import { PublicUrlCreateForm } from "./public-url-create-form";
import { PublicUrlDetails } from "./public-url-details";
import { PublicUrlPasswordDialog, type RevealedPassword } from "./public-url-password-dialog";

interface PublicUrlDialogProps {
  /** A registered domain, or a project's namespace. */
  localDomain: string;
  projectId: string | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * One URL per project, as locally: any domain under the project's namespace opens through the
 * project's wildcard URL, so the dialog creates or shows that one.
 */
export const PublicUrlDialog = ({
  localDomain,
  projectId,
  open,
  onOpenChange,
}: PublicUrlDialogProps) => {
  const { data: bootstrap } = useQuery(bootstrapQueries.read());
  const account = useQuery(cloudQueries.account());
  const signedIn = account.data?.signedIn ?? false;
  const publicUrls = useQuery({ ...cloudQueries.publicUrls(), enabled: signedIn });
  const [revealed, setRevealed] = useState<RevealedPassword | null>(null);
  const namespace =
    projectId === null
      ? null
      : projectNamespaceDomain(projectId, bootstrap?.environment.fixedDomainSuffix ?? "");
  const match = findPublicUrl(publicUrls.data, localDomain, namespace);
  const target = publicUrlTarget(localDomain, namespace);
  const projectName =
    bootstrap?.projects.find((project) => project.id === projectId)?.name ??
    projectId ??
    localDomain;

  // The password and its link open the domain the dialog is about.
  const handleCreated = (result: PublicUrlWithPasswordDto) => {
    if (result.password) {
      const url =
        publicUrlOf(result.publicUrl.url, result.publicUrl.slug, target.label) ??
        result.publicUrl.url;
      const link = result.publicUrl.passwordLink;
      setRevealed({
        url,
        password: result.password,
        passwordLink: link ? linkOnHost(link, url) : null,
      });
    } else {
      toast.success("Public URL created");
    }
  };

  const content = (() => {
    if (account.isPending) return <PublicUrlLoading label="Checking Localify Cloud…" />;
    if (account.error) return <PublicUrlNotice error>{account.error.message}</PublicUrlNotice>;
    if (!signedIn) {
      return (
        <PublicUrlNotice>
          Sign in to Localify Cloud to open this domain from any device.{" "}
          <Link
            to="/settings"
            hash={SETTINGS_CLOUD_ANCHOR}
            onClick={() => onOpenChange(false)}
            className="font-medium text-foreground underline underline-offset-3"
          >
            Open Localify Cloud settings
          </Link>
        </PublicUrlNotice>
      );
    }
    if (publicUrls.isPending) return <PublicUrlLoading label="Loading public URLs…" />;
    if (publicUrls.error) {
      return (
        <PublicUrlNotice
          error
          action={
            <Button
              type="button"
              variant="outline"
              loading={publicUrls.isFetching}
              onClick={() => void publicUrls.refetch()}
            >
              Try again
            </Button>
          }
        >
          {publicUrls.error.message}
        </PublicUrlNotice>
      );
    }
    if (!match) {
      return (
        <PublicUrlCreateForm
          localDomain={localDomain}
          target={target}
          projectName={projectName}
          onCreated={handleCreated}
        />
      );
    }
    return (
      <PublicUrlDetails
        match={match}
        localDomain={localDomain}
        isProjectUrl={match.publicUrl.localDomain.toLowerCase() === namespace}
        projectName={projectName}
        account={account.data}
        onPasswordRevealed={setRevealed}
      />
    );
  })();

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      {/* Scrolls inside itself so its sticky footer stays at its bottom, and
          hides while a confirmation or the password sits on top of it. */}
      <DialogContent className="max-h-[calc(100vh-2rem)] overflow-y-auto data-nested-dialog-open:invisible sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Public URL</DialogTitle>
          <DialogDescription>
            Open <span className="font-mono text-foreground">{localDomain}</span> from any device,
            through Localify Cloud.
          </DialogDescription>
        </DialogHeader>
        {content}
        <PublicUrlPasswordDialog revealed={revealed} onClose={() => setRevealed(null)} />
      </DialogContent>
    </Dialog>
  );
};

const PublicUrlLoading = ({ label }: { label: string }) => (
  <>
    <DialogBody>
      <p className="flex items-center gap-2 text-sm text-muted-foreground">
        <SpinnerIcon className="size-4 animate-spin" />
        {label}
      </p>
    </DialogBody>
    <DialogFooter>
      <DialogClose render={<Button variant="outline" className="sm:ml-auto" />}>Close</DialogClose>
    </DialogFooter>
  </>
);

const PublicUrlNotice = ({
  children,
  error = false,
  action,
}: {
  children: ReactNode;
  error?: boolean;
  action?: ReactNode;
}) => (
  <>
    <DialogBody>
      <p
        role={error ? "alert" : undefined}
        className={error ? "text-sm text-destructive" : "text-sm text-muted-foreground"}
      >
        {children}
      </p>
    </DialogBody>
    <DialogFooter>
      <DialogClose
        render={<Button variant="outline" className={action ? undefined : "sm:ml-auto"} />}
      >
        Close
      </DialogClose>
      {action}
    </DialogFooter>
  </>
);
