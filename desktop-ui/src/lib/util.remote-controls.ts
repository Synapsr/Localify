import type {
  RemoteAccessEventDto,
  RemoteAccessOutcome,
  RemoteAccessWants,
  RemoteControlAction,
  RemoteControlOutcome,
  RemoteControlRequest,
  RemoteInventoryItemDto,
} from "@/types/localify";

import { remoteErrorMessage } from "./util.remote.ts";

// Remote Start, Stop and Restart, the quick actions and access requests, in the words of
// docs/remote-machines.md ("Desktop app", "Asking for access", "State machine"). Pure, so it
// is tested without a window.

/** What clicking a control does. */
export type QuickAction =
  /** Allowed: send it. */
  | { kind: "run" }
  /** A legacy run: `Relaunch on B` after a confirmation, asking for `access` first when set. */
  | { kind: "relaunch"; access: RemoteAccessWants | null }
  /** `Ask B`, with the command to run there. */
  | { kind: "ask"; wants: RemoteAccessWants }
  /** A target of protocol 1: only the command, under "Update Localify on B to ask from here." */
  | { kind: "command"; wants: RemoteAccessWants }
  /** Why, with nothing to do from here. */
  | { kind: "message"; text: string };

/**
 * The switch a refusal asks for, when asking can fix it. With `control` off, an action that
 * would run in the background on the target asks for `backgroundStarts`, which turns
 * `control` on too: asking for `control` alone would get `background_disabled` next, and a
 * second request within 10 minutes is refused `too_many`.
 */
export function accessWants(
  code: string | undefined,
  background: boolean,
): RemoteAccessWants | null {
  switch (code) {
    case "control_disabled":
      return background ? "backgroundStarts" : "control";
    case "background_disabled":
      return "backgroundStarts";
    default:
      return null;
  }
}

/**
 * Whether `action` would run in the background on the target (docs/remote-machines.md, "Who
 * acts"): a start while its app is absent, or a restart of a background run.
 */
export function runsInBackground(
  item: RemoteInventoryItemDto,
  action: RemoteControlAction,
  appRunning: boolean,
): boolean {
  switch (action) {
    case "start":
      return !appRunning;
    case "restart":
      return item.owner === "background";
    case "stop":
      return false;
  }
}

/** What turns the switches on, run on the target. */
export function accessCommand(wants: RemoteAccessWants): string {
  return wants === "control"
    ? "localify machines settings --control on"
    : "localify machines settings --control on --background-starts on";
}

/**
 * What a control of `item` offers, from the snapshot's `controls` and the session's protocol
 * (docs/remote-machines.md, "Quick actions"), and whether the target's app runs. Restart of a
 * run with a `legacyId`, and any control refused `legacy_run` on one, is a relaunch, even when
 * `controls.restart` allows it.
 */
export function quickAction(
  item: RemoteInventoryItemDto,
  action: RemoteControlAction,
  proto: number | null,
  machine: string,
  appRunning: boolean,
): QuickAction {
  const control = item.controls[action];
  const legacyId = item.run?.legacyId;
  if (legacyId && (action === "restart" || control.code === "legacy_run")) {
    if (!asksAccess(proto)) return { kind: "message", text: `Update Localify on ${machine}.` };
    // A relaunch is a restart: the restart's checks decide.
    const restart = item.controls.restart;
    if (restart.allowed) return { kind: "relaunch", access: null };
    const wants = accessWants(restart.code, runsInBackground(item, "restart", appRunning));
    if (wants) return { kind: "relaunch", access: wants };
    return { kind: "message", text: refusalText(restart.code, machine) };
  }
  if (control.allowed) return { kind: "run" };
  if (control.code === "legacy_run") return { kind: "message", text: `Relaunch it on ${machine}.` };
  const wants = accessWants(control.code, runsInBackground(item, action, appRunning));
  if (wants) return asksAccess(proto) ? { kind: "ask", wants } : { kind: "command", wants };
  return { kind: "message", text: refusalText(control.code, machine) };
}

/** Protocol 2 and later carry access requests and relaunches. */
function asksAccess(proto: number | null): boolean {
  return proto !== null && proto >= 2;
}

function refusalText(code: string | undefined, machine: string): string {
  return remoteErrorMessage(code ?? "", machine, `Not available on ${machine}.`);
}

/** The legacy run a person confirmed relaunching, as the snapshot showed it. */
export interface RelaunchConfirmation {
  legacyId: string;
  configRev: string;
}

export function relaunchConfirmation(item: RemoteInventoryItemDto): RelaunchConfirmation | null {
  const legacyId = item.run?.legacyId;
  return legacyId ? { legacyId, configRev: item.service.configRev } : null;
}

/**
 * Once access is granted, a relaunch goes ahead only while the snapshot still shows the run
 * and the configuration the person confirmed; otherwise it asks again.
 */
export function relaunchStillConfirmed(
  confirmed: RelaunchConfirmation,
  item: RemoteInventoryItemDto,
): boolean {
  return (
    item.run?.legacyId === confirmed.legacyId && item.service.configRev === confirmed.configRev
  );
}

/**
 * A control as the snapshot showed the service: its ids, and what it expects. A relaunch is
 * a restart naming the legacy run, with no run id.
 */
export function controlRequest(
  item: RemoteInventoryItemDto,
  action: RemoteControlAction,
  relaunch: RelaunchConfirmation | null = null,
): RemoteControlRequest {
  return {
    action,
    service: {
      projectId: item.project.id,
      environment: item.environment.slug,
      serviceId: item.service.id,
    },
    // Start takes the effective default; a restart keeps the run's profile.
    profile: null,
    expect:
      action === "restart" && relaunch
        ? { runId: null, configRev: relaunch.configRev, legacyId: relaunch.legacyId }
        : { runId: item.run?.runId ?? null, configRev: item.service.configRev },
  };
}

/** Why the controls are off while the session is down: they are never queued. */
export function sessionDownReason(input: {
  machine: string;
  machineLabel: string | null;
  phase: "connecting" | "ready" | "lost";
  proto: number | null;
}): string | null {
  if (input.machineLabel) return `${input.machine}: ${input.machineLabel}`;
  if (input.phase === "lost") return `The connection to ${input.machine} is down.`;
  if (input.phase === "connecting" || input.proto === null) {
    return `Connecting to ${input.machine}…`;
  }
  return null;
}

/** A control on its way, as the chip says it. */
export type ControlPhase = "sent" | "dispatched" | "started" | "unknown";

export function controlProgressText(
  action: RemoteControlAction,
  phase: ControlPhase,
  machine: string,
  appRunning: boolean,
): string {
  if (phase === "unknown") return `Outcome unknown. Checking ${machine}…`;
  if (action === "stop") return `Stopping on ${machine}…`;
  if (phase === "started") return `Started on ${machine}. Waiting for its port…`;
  if (phase === "dispatched") {
    return appRunning
      ? `Starting in the app on ${machine}…`
      : `Starting in the background on ${machine}…`;
  }
  return action === "restart" ? `Restarting on ${machine}…` : `Starting on ${machine}…`;
}

export interface ControlResultView {
  ok: boolean;
  text: string;
}

/** How a control ended. */
export function controlResultText(
  result: { outcome: RemoteControlOutcome; code: string | null; message: string },
  action: RemoteControlAction,
  service: string,
  machine: string,
): ControlResultView {
  switch (result.outcome) {
    case "done":
      return {
        ok: true,
        text:
          action === "stop"
            ? `${service} stopped on ${machine}.`
            : `${service} is running on ${machine}.`,
      };
    case "starting":
      return { ok: true, text: `Started on ${machine}, not listening yet.` };
    case "failed":
      return { ok: false, text: result.message || `${service} failed to start on ${machine}.` };
    // Never the last event: the daemon keeps asking until a result or `lost`.
    case "unknown":
      return { ok: false, text: controlLostText("unknown", machine) };
    case "refused":
    case "conflict":
      return {
        ok: false,
        text: controlRefusalText(result.code ?? result.outcome, machine, result.message),
      };
  }
}

/**
 * Why a control was refused, by the target or before it was sent: the connection state when
 * no session was open, the target's checks otherwise. A control result carries no
 * `retryAfter`.
 */
function controlRefusalText(code: string, machine: string, message: string): string {
  switch (code) {
    case "busy":
      return `${remoteErrorMessage(code, machine, message)} Try again in a moment.`;
    case "too_many":
      return `This Mac sent ${machine} too many controls lately. Try again later.`;
    default:
      return remoteErrorMessage(code, machine, message);
  }
}

/** A control whose outcome could not be learned, by the `lost` event's code. */
export function controlLostText(code: string, machine: string): string {
  switch (code) {
    case "not_shared":
      return `${machine} stopped sharing before it said what happened.`;
    case "not_paired":
    case "key_mismatch":
      return `${machine} didn't say what happened. ${remoteErrorMessage(code, machine, "")}`;
    default:
      return `${machine} didn't say what happened. The list shows its current state.`;
  }
}

/** An access request, as the card shows it. */
export type AccessView =
  | { step: "waiting"; text: string }
  | { step: "ended"; ok: boolean; text: string };

export function accessOutcomeText(
  outcome: RemoteAccessOutcome,
  wants: RemoteAccessWants,
  machine: string,
): AccessView {
  switch (outcome) {
    case "granted":
      return {
        step: "ended",
        ok: true,
        text:
          wants === "control"
            ? `${machine} allowed remote controls.`
            : `${machine} allowed remote controls and background starts.`,
      };
    case "declined":
      return { step: "ended", ok: false, text: `${machine} didn't allow it.` };
    case "nobody":
      return {
        step: "ended",
        ok: false,
        text: `Nobody can answer on ${machine}: Localify isn't open there.`,
      };
    case "expired":
      return { step: "ended", ok: false, text: `Nobody answered on ${machine} in time.` };
    case "cancelled":
      return {
        step: "ended",
        ok: false,
        text: `The request ended on ${machine} before an answer.`,
      };
  }
}

/** `9 min`, or `45 s` under a minute. */
export function formatWait(seconds: number): string {
  if (seconds < 60) return `${Math.max(1, Math.ceil(seconds))} s`;
  return `${Math.ceil(seconds / 60)} min`;
}

/** What `remote_access` said, as the card shows it. */
export function accessEventView(
  event: RemoteAccessEventDto,
  wants: RemoteAccessWants,
  machine: string,
): AccessView {
  switch (event.event) {
    case "waiting":
      return { step: "waiting", text: `Waiting for an answer on ${machine}…` };
    case "result":
      return accessOutcomeText(event.outcome, wants, machine);
    case "error":
      return { step: "ended", ok: false, text: accessErrorText(event, machine) };
    case "lost":
      return {
        step: "ended",
        ok: false,
        text: `The connection to ${machine} ended before an answer.`,
      };
  }
}

function accessErrorText(
  error: { code: string; message: string; retryAfter?: number },
  machine: string,
): string {
  switch (error.code) {
    case "too_many":
      return error.retryAfter
        ? `You asked ${machine} recently. Try again in ${formatWait(error.retryAfter)}.`
        : `You asked ${machine} recently. Try again later.`;
    case "busy":
      return `${machine} is answering another request.`;
    default:
      return remoteErrorMessage(error.code, machine, error.message);
  }
}

/** `3F2A 91C0`: the first 8 characters of a fingerprint, as the access dialog names a peer. */
export function shortFingerprint(fingerprint: string): string {
  const hex = fingerprint
    .replace(/[^0-9a-f]/gi, "")
    .slice(0, 8)
    .toUpperCase();
  return hex.length > 4 ? `${hex.slice(0, 4)} ${hex.slice(4)}` : hex;
}

/** "teo-laptop (3F2A 91C0) asks for remote controls.", from the local pin. */
export function accessRequestTitle(name: string, fingerprint: string): string {
  const short = shortFingerprint(fingerprint);
  return short
    ? `${name} (${short}) asks for remote controls.`
    : `${name} asks for remote controls.`;
}

/** An access request on the target, as its dialog shows it. */
export type AccessPrompt<R extends { requestId: string }> = R & {
  /** The answer is on its way: the buttons are off. */
  answering: boolean;
  /** Why the last answer could not be sent; the request still waits. */
  error: string | null;
};

/** A request the daemon sent, queued once. */
export function accessPromptAdded<R extends { requestId: string }>(
  queue: AccessPrompt<R>[],
  request: R,
): AccessPrompt<R>[] {
  if (queue.some((prompt) => prompt.requestId === request.requestId)) return queue;
  return [...queue, { ...request, answering: false, error: null }];
}

/** A request answered, or closed by the daemon (`access_closed`): its dialog goes. */
export function accessPromptClosed<R extends { requestId: string }>(
  queue: AccessPrompt<R>[],
  requestId: string,
): AccessPrompt<R>[] {
  return queue.filter((prompt) => prompt.requestId !== requestId);
}

/** The answer is sent: the dialog stays, with its buttons off, until it is through. */
export function accessPromptAnswering<R extends { requestId: string }>(
  queue: AccessPrompt<R>[],
  requestId: string,
): AccessPrompt<R>[] {
  return queue.map((prompt) =>
    prompt.requestId === requestId ? { ...prompt, answering: true, error: null } : prompt,
  );
}

/**
 * The answer could not be sent: the request can be answered again, with the error shown, as
 * long as it still waits. One the daemon closed meanwhile stays gone.
 */
export function accessPromptFailed<R extends { requestId: string }>(
  queue: AccessPrompt<R>[],
  requestId: string,
  error: string,
): AccessPrompt<R>[] {
  return queue.map((prompt) =>
    prompt.requestId === requestId ? { ...prompt, answering: false, error } : prompt,
  );
}
