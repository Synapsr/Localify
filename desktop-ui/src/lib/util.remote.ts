import type {
  RemoteInventoryItemDto,
  RemoteMachineDto,
  RemoteOwner,
  RemoteStatusDto,
} from "@/types/localify";

// What the app says about remote machines, in the words of docs/remote-machines.md
// ("Desktop app", "Errors"). Pure, so it is tested without a window.

/** The Remote access card's id, for links to `/settings#remote-access`. */
export const SETTINGS_REMOTE_ANCHOR = "remote-access";

export type RemoteTone = "ok" | "pending" | "warn" | "error" | "off";

/** `10:42`, from an RFC 3339 time. */
export function formatClock(value: string | number | null | undefined): string {
  if (value === null || value === undefined) return "";
  const date = typeof value === "number" ? new Date(value * 1000) : new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

export interface RelayView {
  label: string;
  detail: string | null;
  tone: RemoteTone;
  /** Offer `Retry now`. */
  retry: boolean;
}

/** This machine's relay connection, as the switcher's header says it. */
export function describeRelay(status: RemoteStatusDto, now: number = Date.now()): RelayView {
  switch (status.relay) {
    case "connected":
      return { label: "Remote access: connected", detail: null, tone: "ok", retry: false };
    case "connecting":
      return { label: "Connecting…", detail: null, tone: "pending", retry: false };
    case "reconnecting": {
      const at = status.retryAt ? new Date(status.retryAt).getTime() : Number.NaN;
      const seconds = Math.ceil((at - now) / 1000);
      const label =
        Number.isFinite(seconds) && seconds > 0
          ? `Reconnecting… (in ${seconds} s)`
          : "Reconnecting…";
      return { label, detail: status.blocked?.message ?? null, tone: "pending", retry: true };
    }
    case "blocked":
      return { label: "Off", detail: status.blocked?.message ?? null, tone: "off", retry: false };
    case "off":
      return {
        label: "Off",
        detail: "Sign in to Localify Cloud to see your other machines.",
        tone: "off",
        retry: false,
      };
  }
}

/** Whether remote access is off because the plan lacks it. */
export function needsPro(status: RemoteStatusDto | undefined): boolean {
  return status?.blocked?.code === "subscription_required";
}

export interface MachineView {
  /** Null for a connected machine: its name, with no badge. */
  label: string | null;
  detail: string | null;
  tone: RemoteTone;
  action: "pair" | "unpair" | "upgrade" | null;
  /** Its projects can be shown, or at least asked for. */
  viewable: boolean;
}

/** A machine's state in the switcher (docs/remote-machines.md, "Machine switcher"). */
export function describeMachine(machine: RemoteMachineDto): MachineView {
  const view = (
    label: string | null,
    tone: RemoteTone,
    rest: Partial<Pick<MachineView, "detail" | "action">> = {},
  ): MachineView => ({
    label,
    tone,
    detail: rest.detail ?? null,
    action: rest.action ?? null,
    viewable:
      machine.paired &&
      !["key_mismatch", "signed_out", "unavailable", "incompatible"].includes(machine.state),
  });
  switch (machine.state) {
    case "offline": {
      const since = formatClock(machine.since);
      return view("Offline", "off", { detail: since ? `since ${since}` : null });
    }
    case "connecting":
      return view("Connecting…", "pending");
    case "reconnecting":
      return view("Reconnecting…", "pending");
    case "connected":
      return view(null, "ok");
    case "not_responding":
      return view("Not responding", "warn");
    case "incompatible":
      return view("Update needed", "warn", {
        detail: `${machine.name} runs a version of Localify that can't talk to this one.${
          machine.version ? ` It runs ${machine.version}.` : ""
        }`,
      });
    case "not_paired":
      return view("Not paired", "off", { action: "pair" });
    case "key_mismatch":
      return view("Key mismatch", "error", {
        detail: "Don't trust this machine until you pair it again.",
        action: "unpair",
      });
    case "not_sharing":
      return view("Paired · not sharing", "off");
    case "signed_out":
      return view("Signed out", "off", { action: "unpair" });
    case "unavailable":
      return view("Remote access needs Localify Cloud Pro", "off", { action: "upgrade" });
  }
}

/** What the viewer says for a target's error code. */
export function remoteErrorMessage(code: string, machine: string, fallback: string): string {
  switch (code) {
    case "not_paired":
      return `This Mac is not paired with ${machine}. Pair them again.`;
    case "key_mismatch":
      return "Don't trust this machine until you pair it again.";
    case "not_shared":
      return `${machine} doesn't share its projects. Turn on sharing in Localify on ${machine}.`;
    case "control_disabled":
      return `${machine} doesn't allow remote controls.`;
    case "background_disabled":
      return `Localify isn't open on ${machine}, and ${machine} doesn't allow background starts.`;
    case "not_owned":
      return `This runs in a terminal on ${machine}. Stop it there.`;
    case "legacy_run":
      return "Relaunch it on that machine to enable remote control.";
    case "owner_gone":
      return "Stop it, then start it again.";
    case "refused_profile":
      return `Start it on ${machine} to use this profile.`;
    case "unknown_service":
      return `This service no longer exists on ${machine}.`;
    case "conflict":
      return `${machine} changed in the meantime.`;
    case "expired":
      return `The request took too long to reach ${machine}. Nothing was started.`;
    case "busy":
      return `Something else is starting or stopping it on ${machine}.`;
    case "unauthorized":
    case "lease_expired":
      return "Remote access is not available right now.";
    case "subscription_required":
      return "Remote access needs Localify Cloud Pro.";
    case "unsupported":
      return `Update Localify on ${machine}.`;
    case "peer_offline":
      return `${machine} is offline.`;
    case "revoked":
      return `${machine} was removed from the account.`;
    case "unpaired":
      return `${machine} is no longer paired with this Mac.`;
    default:
      return fallback;
  }
}

export const PAIRING_CANCELLED = "Pairing cancelled. Nothing was saved.";

/** Why a pairing this Mac asked for did not happen (docs/remote-machines.md, "Pairing"). */
export function pairingCancelledMessage(reason: string, message: string, machine: string): string {
  switch (reason) {
    case "nobody_to_confirm":
      return `Nobody can confirm on ${machine}. Open Localify on ${machine}, or run \`localify machines pair --accept\` there, then try again.`;
    case "busy":
      return `${machine} is busy with another pairing. Try again in a moment.`;
    default:
      return !message || message === PAIRING_CANCELLED
        ? PAIRING_CANCELLED
        : `${PAIRING_CANCELLED} ${message}`;
  }
}

/** Codes after which asking again cannot help until something changes. */
export function isFinalCode(code: string): boolean {
  return [
    "not_paired",
    "key_mismatch",
    "revoked",
    "unpaired",
    "output_unavailable",
    "bad_request",
    "subscription_required",
    "banned",
    "signed_out",
  ].includes(code);
}

export function ownerLabel(owner: RemoteOwner | null): string | null {
  switch (owner) {
    case "app":
      return "In the app";
    case "background":
      return "In the background";
    case "terminal":
      return "In a terminal";
    case "unknown":
      return "Unknown process";
    case null:
      return null;
  }
}

export interface ServiceChip {
  name: "Process" | "Port" | "Output" | "Public URL";
  value: string;
  tone: RemoteTone;
}

/** The four states of a service, each from its own source, so each one is shown. */
export function serviceChips(item: RemoteInventoryItemDto): ServiceChip[] {
  return [processChip(item), portChip(item), outputChip(item), publicUrlChip(item)];
}

function processChip(item: RemoteInventoryItemDto): ServiceChip {
  if (item.process === "running") {
    return item.port.reachable === false
      ? { name: "Process", value: "Starting", tone: "pending" }
      : { name: "Process", value: "Running", tone: "ok" };
  }
  const exit = item.run?.exit;
  if (exit && exit.code !== null && exit.code !== 0) {
    return { name: "Process", value: `Exited with ${exit.code}`, tone: "warn" };
  }
  return { name: "Process", value: "Stopped", tone: "off" };
}

function portChip(item: RemoteInventoryItemDto): ServiceChip {
  const port = `:${item.port.number}`;
  switch (item.port.reachable) {
    case true:
      return { name: "Port", value: `${port} reachable`, tone: "ok" };
    case false:
      return { name: "Port", value: `${port} not reachable`, tone: "off" };
    default:
      return { name: "Port", value: `${port} not checked`, tone: "off" };
  }
}

function outputChip(item: RemoteInventoryItemDto): ServiceChip {
  if (item.output.available) return { name: "Output", value: "Available", tone: "ok" };
  switch (item.output.reason) {
    case "terminal":
      return { name: "Output", value: "In a terminal", tone: "off" };
    case "unknown_owner":
      return { name: "Output", value: "Unknown process", tone: "off" };
    case "legacy_run":
      return { name: "Output", value: "Relaunch to share", tone: "warn" };
    default:
      return { name: "Output", value: "No run yet", tone: "off" };
  }
}

function publicUrlChip(item: RemoteInventoryItemDto): ServiceChip {
  const url = item.publicUrl;
  if (!url) return { name: "Public URL", value: "None", tone: "off" };
  if (url.serving) return { name: "Public URL", value: "Serving", tone: "ok" };
  const reason = {
    suspended: "Suspended",
    taken_down: "Taken down",
    served_elsewhere: "Served elsewhere",
    tunnel_down: "Tunnel down",
  } as const;
  return {
    name: "Public URL",
    value: url.reason ? reason[url.reason] : "Not serving",
    tone: "warn",
  };
}

/** A target's `.localify` URL, which resolves on that machine only: `web.louez.localify · on B`. */
export function remoteUrlLabel(localUrl: string, machine: string): string {
  const host = localUrl.replace(/^[a-z]+:\/\//i, "").replace(/\/$/, "");
  return `${host} · on ${machine}`;
}

export interface RemoteEnvironmentGroup {
  key: string;
  name: string;
  primary: boolean;
  branch: string | null;
  services: RemoteInventoryItemDto[];
}

export interface RemoteProjectGroup {
  id: string;
  name: string;
  environments: RemoteEnvironmentGroup[];
}

/**
 * Projects, then environments, then services, in the target's order. With a query, only the
 * services whose project, environment, name or URLs match.
 */
export function groupInventory(items: RemoteInventoryItemDto[], query = ""): RemoteProjectGroup[] {
  const needle = query.trim().toLowerCase();
  const matches = (item: RemoteInventoryItemDto) =>
    !needle ||
    [
      item.project.name,
      item.environment.name,
      item.environment.branch ?? "",
      item.service.name,
      item.localUrl,
      item.publicUrl?.url ?? "",
    ].some((value) => value.toLowerCase().includes(needle));
  const projects: RemoteProjectGroup[] = [];
  for (const item of items.filter(matches)) {
    let project = projects.find((group) => group.id === item.project.id);
    if (!project) {
      project = { id: item.project.id, name: item.project.name, environments: [] };
      projects.push(project);
    }
    const key = item.environment.slug ?? "";
    let environment = project.environments.find((group) => group.key === key);
    if (!environment) {
      environment = {
        key,
        name: item.environment.name,
        primary: item.environment.primary,
        branch: item.environment.branch,
        services: [],
      };
      project.environments.push(environment);
    }
    environment.services.push(item);
  }
  return projects;
}

/** One service on a target, across snapshots. */
export function serviceKey(item: RemoteInventoryItemDto): string {
  return `${item.project.id}/${item.environment.slug ?? ""}/${item.service.id}`;
}
