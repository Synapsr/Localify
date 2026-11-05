"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useLocation } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { toast } from "sonner";

import type { CloudAccountDto, CloudSignInStartDto } from "@/types/localify";

import {
  CheckIcon,
  CloudIcon,
  CopyIcon,
  ExternalLinkIcon,
  GithubIcon,
  InfoCircleIcon,
  SpinnerIcon,
} from "@/components/icons";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { bootstrapQueries, cloudMutations, cloudQueries } from "@/lib/queries";
import { tauriClient } from "@/lib/tauri-client";
import { cloudStatusDotClassName, describeTunnel, SETTINGS_CLOUD_ANCHOR } from "@/lib/util.cloud";
import { cn } from "@/lib/utils";

import { SettingsCard } from "./settings-card";

const SLOW_DOWN_SECONDS = 5;

type SignInStopReason = "denied" | "notAllowed" | "expired" | "error";

type SignInFlow =
  | { step: "idle" }
  | { step: "waiting"; start: CloudSignInStartDto; intervalSeconds: number }
  | { step: "stopped"; reason: SignInStopReason; message: string | null };

export const SettingsCloud = () => {
  const queryClient = useQueryClient();
  const { data: bootstrap } = useQuery(bootstrapQueries.read());
  const account = useQuery(cloudQueries.account());
  const startSignIn = useMutation(cloudMutations.startSignIn());
  const { mutateAsync: pollSignIn } = useMutation(cloudMutations.pollSignIn(queryClient));
  const signOut = useMutation(cloudMutations.signOut(queryClient));
  const [flow, setFlow] = useState<SignInFlow>({ step: "idle" });
  const [signOutOpen, setSignOutOpen] = useState(false);
  const hash = useLocation({ select: (location) => location.hash });
  const browser = bootstrap?.launcherPreferences.browser ?? "default";

  useEffect(() => {
    if (hash !== SETTINGS_CLOUD_ANCHOR) return;
    document
      .getElementById(SETTINGS_CLOUD_ANCHOR)
      ?.scrollIntoView({ block: "center", behavior: "smooth" });
  }, [hash]);

  // One poll per run: each answer sets a new flow object, which schedules the next poll.
  useEffect(() => {
    if (flow.step !== "waiting") return;
    let active = true;
    const timer = window.setTimeout(async () => {
      try {
        const result = await pollSignIn({ deviceCode: flow.start.deviceCode });
        if (!active) return;
        switch (result.status) {
          case "pending":
            setFlow({ ...flow });
            return;
          case "slowDown":
            setFlow({ ...flow, intervalSeconds: flow.intervalSeconds + SLOW_DOWN_SECONDS });
            return;
          case "signedIn":
            if (result.account) {
              queryClient.setQueryData(cloudQueries.account().queryKey, result.account);
            }
            setFlow({ step: "idle" });
            toast.success("Signed in to Localify Cloud", {
              description: result.account?.login ? `As ${result.account.login}.` : undefined,
            });
            return;
          case "denied":
          case "notAllowed":
          case "expired":
            setFlow({ step: "stopped", reason: result.status, message: result.message });
            return;
        }
      } catch (error) {
        if (!active) return;
        setFlow({
          step: "stopped",
          reason: "error",
          message: error instanceof Error ? error.message : String(error),
        });
      }
    }, flow.intervalSeconds * 1000);

    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [flow, pollSignIn, queryClient]);

  const openVerificationPage = async (url: string) => {
    try {
      await tauriClient.openProjectUrl({ url, browser });
    } catch (error) {
      const description = error instanceof Error ? error.message : "Unable to open this URL.";
      toast.error("Open in browser failed", { description });
    }
  };

  const handleSignIn = async () => {
    try {
      const start = await startSignIn.mutateAsync();
      setFlow({ step: "waiting", start, intervalSeconds: Math.max(start.interval, 1) });
      void openVerificationPage(start.verificationUri);
    } catch (error) {
      const description = error instanceof Error ? error.message : String(error);
      toast.error("Could not start sign-in", { description });
    }
  };

  const handleSignOut = async () => {
    try {
      const result = await signOut.mutateAsync();
      queryClient.setQueryData(cloudQueries.account().queryKey, result);
      setSignOutOpen(false);
      toast.success("Signed out of Localify Cloud");
    } catch (error) {
      const description = error instanceof Error ? error.message : String(error);
      toast.error("Could not sign out", { description });
    }
  };

  const signedIn = account.data?.signedIn ?? false;
  const waiting = flow.step === "waiting";

  const description = (() => {
    if (account.isPending) return "Checking your account…";
    if (account.error) return account.error.message;
    if (signedIn && account.data) return signedInSummary(account.data);
    return "Open a local domain from any device, at a public URL. Invite-only for now.";
  })();

  const action = (() => {
    if (account.isPending || waiting) return null;
    if (signedIn) {
      return (
        <Button variant="outline" onClick={() => setSignOutOpen(true)}>
          Sign out
        </Button>
      );
    }
    return (
      <Button
        variant="secondary"
        loading={startSignIn.isPending}
        onClick={() => void handleSignIn()}
      >
        {startSignIn.isPending ? null : <GithubIcon />}
        Sign in with GitHub
      </Button>
    );
  })();

  return (
    <>
      <SettingsCard
        id={SETTINGS_CLOUD_ANCHOR}
        icon={<CloudIcon />}
        title="Localify Cloud"
        description={description}
        action={action}
      >
        {flow.step === "waiting" ? (
          <SignInCodePanel
            start={flow.start}
            onOpen={() => void openVerificationPage(flow.start.verificationUri)}
            onCancel={() => setFlow({ step: "idle" })}
          />
        ) : null}

        {flow.step === "stopped" && !signedIn ? (
          <SignInStoppedNotice reason={flow.reason} message={flow.message} />
        ) : null}

        {signedIn && account.data ? <AccountDetails account={account.data} /> : null}
      </SettingsCard>

      <Dialog open={signOutOpen} onOpenChange={setSignOutOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>Sign out of Localify Cloud?</DialogTitle>
            <DialogDescription>
              Public URLs served by this Mac go offline until a signed-in Mac serves them. They are
              not deleted.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <DialogClose render={<Button variant="outline" />} disabled={signOut.isPending}>
              Cancel
            </DialogClose>
            <Button
              type="button"
              variant="danger"
              loading={signOut.isPending}
              onClick={() => void handleSignOut()}
            >
              Sign out
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
};

function signedInSummary(account: CloudAccountDto): string {
  const who = account.login ? `Signed in as ${account.login}` : "Signed in";
  return account.machineName ? `${who} on ${account.machineName}.` : `${who}.`;
}

const SignInCodePanel = ({
  start,
  onOpen,
  onCancel,
}: {
  start: CloudSignInStartDto;
  onOpen: () => void;
  onCancel: () => void;
}) => {
  const { copy, copied } = useCopyToClipboard();

  return (
    <div className="mt-5 space-y-4 border-t border-border/40 pt-5">
      <div className="space-y-2">
        <div className="text-sm font-medium text-foreground">
          Enter this code at{" "}
          <span className="font-mono">{start.verificationUri.replace(/^https?:\/\//, "")}</span>
        </div>
        <div className="flex items-center gap-2">
          <code className="rounded-lg bg-background px-3 py-1.5 font-mono text-2xl font-semibold tracking-[0.2em] text-foreground ring-1 ring-border/60 select-all">
            {start.userCode}
          </code>
          <Button
            type="button"
            size="icon-sm"
            variant="ghost"
            aria-label={copied ? "Code copied" : "Copy code"}
            onClick={() => void copy(start.userCode)}
          >
            {copied ? <CheckIcon className="text-emerald-500" /> : <CopyIcon />}
          </Button>
        </div>
      </div>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <span className="inline-flex items-center gap-2 text-xs text-muted-foreground">
          <SpinnerIcon className="size-3.5 animate-spin" />
          Waiting for GitHub…
        </span>
        <div className="flex gap-2">
          <Button type="button" variant="outline" onClick={onCancel}>
            Cancel
          </Button>
          <Button type="button" variant="secondary" onClick={onOpen}>
            <ExternalLinkIcon />
            Open GitHub
          </Button>
        </div>
      </div>
    </div>
  );
};

const SignInStoppedNotice = ({
  reason,
  message,
}: {
  reason: SignInStopReason;
  message: string | null;
}) => {
  const copy = {
    denied: {
      title: "Sign-in was cancelled on GitHub",
      description: "Sign in again when you are ready.",
    },
    notAllowed: {
      title: "Localify Cloud is invite-only for now",
      description: message ?? "This GitHub account is not on the invite list yet.",
    },
    expired: {
      title: "The code expired",
      description: "Sign in again to get a new one.",
    },
    error: {
      title: "Sign-in failed",
      description: message ?? "Try again.",
    },
  }[reason];

  return (
    <div className="mt-5 flex items-start gap-3 rounded-xl border border-border/60 bg-background/70 p-3">
      <span
        aria-hidden
        className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-lg bg-amber-500/10 text-amber-700 ring-1 ring-amber-500/20 dark:text-amber-300 [&>svg]:size-4"
      >
        <InfoCircleIcon />
      </span>
      <div className="min-w-0 flex-1 space-y-1" role="alert">
        <div className="text-sm font-medium text-foreground">{copy.title}</div>
        <div className="text-xs leading-normal text-muted-foreground">{copy.description}</div>
      </div>
    </div>
  );
};

const AccountDetails = ({ account }: { account: CloudAccountDto }) => {
  const tunnel = describeTunnel(account.tunnel);

  return (
    <dl className="mt-5 grid gap-4 border-t border-border/40 pt-5 sm:grid-cols-3">
      <div className="min-w-0 space-y-0.5">
        <dt className="text-xs text-muted-foreground">GitHub account</dt>
        <dd className="truncate text-sm text-foreground">{account.login ?? "Unknown"}</dd>
      </div>
      <div className="min-w-0 space-y-0.5">
        <dt className="text-xs text-muted-foreground">This Mac</dt>
        <dd className="truncate text-sm text-foreground">{account.machineName ?? "Unknown"}</dd>
      </div>
      <div className="min-w-0 space-y-0.5">
        <dt className="text-xs text-muted-foreground">This Mac's tunnel</dt>
        <dd className="flex items-center gap-1.5 text-sm text-foreground">
          {tunnel.tone ? (
            <span
              aria-hidden
              className={cn("size-2 shrink-0 rounded-full", cloudStatusDotClassName[tunnel.tone])}
            />
          ) : null}
          {tunnel.label}
        </dd>
        {tunnel.detail ? (
          <dd className="text-xs leading-normal text-muted-foreground">{tunnel.detail}</dd>
        ) : null}
      </div>
    </dl>
  );
};
