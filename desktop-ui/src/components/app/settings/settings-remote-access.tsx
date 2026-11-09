"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useLocation } from "@tanstack/react-router";
import { type ReactNode, useEffect, useState } from "react";
import { toast } from "sonner";

import type { RemoteMachineDto, RemoteSettingsDto } from "@/types/localify";

import { CloudSubscribeAction } from "@/components/app/cloud/cloud-subscribe-action";
import { remoteToneDot } from "@/components/app/remote/machine-switcher";
import { ServersIcon } from "@/components/icons";
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
import { Switch } from "@/components/ui/switch";
import { cloudQueries, remoteMutations, remoteQueries } from "@/lib/queries";
import {
  describeMachine,
  describeRelay,
  needsPro,
  SETTINGS_REMOTE_ANCHOR,
} from "@/lib/util.remote";
import { cn } from "@/lib/utils";
import { useRemote } from "@/providers/remote-provider";

import { SettingsCard } from "./settings-card";

/**
 * Settings → Remote access (docs/remote-machines.md, "Desktop app"): what paired machines may
 * see and do here, all off by default, this Mac's fingerprint, and the paired machines.
 */
export const SettingsRemoteAccess = () => {
  const account = useQuery(cloudQueries.account());
  const status = useQuery(remoteQueries.status());
  const hash = useLocation({ select: (location) => location.hash });
  const signedIn = account.data?.signedIn ?? false;

  useEffect(() => {
    if (hash !== SETTINGS_REMOTE_ANCHOR) return;
    document
      .getElementById(SETTINGS_REMOTE_ANCHOR)
      ?.scrollIntoView({ block: "center", behavior: "smooth" });
  }, [hash]);

  const description = (() => {
    if (status.error) return status.error.message;
    if (!signedIn) {
      return "See your other machines' services, public URLs and output from this Mac. Sign in to Localify Cloud first.";
    }
    if (needsPro(status.data)) return "Remote access needs Localify Cloud Pro.";
    return status.data ? describeRelay(status.data).label : "Checking remote access…";
  })();

  return (
    <SettingsCard
      id={SETTINGS_REMOTE_ANCHOR}
      icon={<ServersIcon />}
      title="Remote access"
      description={description}
      action={signedIn && needsPro(status.data) ? <UpgradeAction /> : null}
    >
      {signedIn && status.data ? (
        <>
          <Switches settings={status.data.settings} />
          {status.data.fingerprint ? (
            <div className="mt-5 border-t border-border/40 pt-5">
              <div className="text-xs text-muted-foreground">This Mac's fingerprint</div>
              <div className="mt-0.5 font-mono text-sm text-foreground select-all">
                {status.data.fingerprint}
              </div>
            </div>
          ) : null}
          <Machines lapsed={needsPro(status.data)} />
          <Suppressed />
        </>
      ) : null}
    </SettingsCard>
  );
};

function UpgradeAction() {
  const plan = useQuery(cloudQueries.plan());
  return <CloudSubscribeAction plan={plan.data} />;
}

function Switches({ settings }: { settings: RemoteSettingsDto }) {
  const queryClient = useQueryClient();
  const change = useMutation(remoteMutations.changeSettings(queryClient));
  const set = (request: Partial<RemoteSettingsDto>) =>
    change.mutate(request, {
      onError: (error) =>
        toast.error("Could not change remote access", { description: error.message }),
    });

  return (
    <div className="mt-5 space-y-4 border-t border-border/40 pt-5">
      <SwitchRow
        label="Share projects and output with paired machines"
        checked={settings.share}
        disabled={change.isPending}
        onChange={(share) => set({ share })}
        note="Output can contain secrets your services print, and what you type into service tabs."
      />
      <SwitchRow
        label="Allow paired machines to start, stop and restart services"
        checked={settings.control}
        disabled={change.isPending || !settings.share}
        onChange={(control) => set({ control })}
        note={settings.share ? undefined : "Needs sharing."}
      />
      <SwitchRow
        label="Allow background starts when the app is closed"
        checked={settings.backgroundStarts}
        disabled={change.isPending || !settings.control}
        onChange={(backgroundStarts) => set({ backgroundStarts })}
        note={settings.control ? undefined : "Needs remote controls."}
      />
    </div>
  );
}

function SwitchRow({
  label,
  checked,
  disabled,
  onChange,
  note,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
  note?: ReactNode;
}) {
  return (
    <label className="flex items-start gap-3">
      <span className="min-w-0 flex-1">
        <span className="block text-sm text-foreground">{label}</span>
        {note ? (
          <span className="mt-0.5 block text-xs leading-normal text-muted-foreground">{note}</span>
        ) : null}
      </span>
      <Switch
        checked={checked}
        disabled={disabled}
        onCheckedChange={(next) => onChange(next)}
        aria-label={label}
      />
    </label>
  );
}

/** With remote access lapsed, the roster is unknown: only pins are listed. */
function Machines({ lapsed }: { lapsed: boolean }) {
  const machines = useQuery(remoteQueries.machines());
  const others = machines.data?.filter((machine) => !machine.self) ?? [];
  const paired = others.filter((machine) => machine.paired);
  const unpaired = others.filter((machine) => !machine.paired);
  if (lapsed && others.length === 0) return null;

  return (
    <div className="mt-5 space-y-3 border-t border-border/40 pt-5">
      <div className="text-xs text-muted-foreground">Paired machines</div>
      {paired.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          {others.length === 0
            ? "No other machine of this account uses Localify yet. Sign in on another Mac to see it here."
            : "None yet. Pair one below: each side confirms the same code."}
        </p>
      ) : (
        <ul className="space-y-2">
          {paired.map((machine) => (
            <PairedMachine key={machine.id} machine={machine} />
          ))}
        </ul>
      )}
      {unpaired.length > 0 ? (
        <>
          <div className="pt-2 text-xs text-muted-foreground">Other machines of this account</div>
          <ul className="space-y-2">
            {unpaired.map((machine) => (
              <UnpairedMachine key={machine.id} machine={machine} />
            ))}
          </ul>
        </>
      ) : null}
    </div>
  );
}

/** Machines whose access requests this Mac declines at once, from its access dialog. */
function Suppressed() {
  const queryClient = useQueryClient();
  const suppressed = useQuery(remoteQueries.suppressed());
  const restore = useMutation(remoteMutations.restoreAccess(queryClient));
  if (!suppressed.data?.length) return null;

  return (
    <div className="mt-5 space-y-3 border-t border-border/40 pt-5">
      <div className="text-xs text-muted-foreground">Machines that can't ask for access</div>
      <ul className="space-y-2">
        {suppressed.data.map((machine) => (
          <li key={machine.machineId} className="flex items-center gap-3">
            <span className="min-w-0 flex-1">
              <span className="block truncate text-sm text-foreground">{machine.name}</span>
              <span className="block text-xs leading-normal text-muted-foreground">
                Since {new Date(machine.since).toLocaleDateString()}
              </span>
            </span>
            <Button
              size="sm"
              variant="outline"
              loading={restore.isPending && restore.variables?.machineId === machine.machineId}
              onClick={() =>
                restore.mutate(
                  { machineId: machine.machineId },
                  {
                    onError: (error) =>
                      toast.error(`Could not allow ${machine.name} again`, {
                        description: error.message,
                      }),
                  },
                )
              }
            >
              Allow requests again
            </Button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function MachineName({ machine }: { machine: RemoteMachineDto }) {
  const view = describeMachine(machine);
  return (
    <span className="min-w-0 flex-1">
      <span className="flex items-center gap-1.5 text-sm text-foreground">
        <span aria-hidden className={cn("size-2 rounded-full", remoteToneDot[view.tone])} />
        <span className="truncate">{machine.name}</span>
      </span>
      <span className="block text-xs leading-normal text-muted-foreground">
        {[view.label ?? "Connected", view.detail, machine.fingerprint].filter(Boolean).join(" · ")}
      </span>
    </span>
  );
}

function UnpairedMachine({ machine }: { machine: RemoteMachineDto }) {
  const { startPairing } = useRemote();
  const online = machine.state === "not_paired";
  return (
    <li className="flex items-center gap-3">
      <MachineName machine={machine} />
      <Button size="sm" variant="outline" disabled={!online} onClick={() => startPairing(machine)}>
        Pair
      </Button>
    </li>
  );
}

function PairedMachine({ machine }: { machine: RemoteMachineDto }) {
  const queryClient = useQueryClient();
  const unpair = useMutation(remoteMutations.unpair(queryClient));
  const remove = useMutation(remoteMutations.remove(queryClient));
  const [removing, setRemoving] = useState(false);

  const doUnpair = () =>
    unpair.mutate(
      { machineId: machine.id },
      {
        onSuccess: () => toast.success(`Unpaired ${machine.name}`),
        onError: (error) =>
          toast.error(`Could not unpair ${machine.name}`, { description: error.message }),
      },
    );
  const doRemove = () =>
    remove.mutate(
      { machineId: machine.id },
      {
        onSuccess: () => {
          setRemoving(false);
          toast.success(`Removed ${machine.name} from the account`);
        },
        onError: (error) =>
          toast.error(`Could not remove ${machine.name}`, { description: error.message }),
      },
    );

  return (
    <li className="flex items-center gap-3">
      <MachineName machine={machine} />
      <Button size="sm" variant="outline" loading={unpair.isPending} onClick={doUnpair}>
        Unpair
      </Button>
      {machine.state === "signed_out" ? null : (
        <Button size="sm" variant="danger" onClick={() => setRemoving(true)}>
          Remove from account
        </Button>
      )}
      <Dialog open={removing} onOpenChange={setRemoving}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>Remove {machine.name} from the account?</DialogTitle>
            <DialogDescription>
              {machine.name} is signed out of Localify Cloud at once, and every machine forgets its
              pairing. Its services keep running. Sign in on it again to use it here.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <DialogClose render={<Button variant="outline" />} disabled={remove.isPending}>
              Cancel
            </DialogClose>
            <Button variant="danger" loading={remove.isPending} onClick={doRemove}>
              Remove from account
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </li>
  );
}
