import { useEffect, useRef, useState } from "react";

import type { RemoteFollowEventDto } from "@/types/localify";

import { useWindowVisible } from "@/hooks/use-window-visible";
import { openRemoteStream } from "@/lib/remote-stream";
import { formatClock, isFinalCode } from "@/lib/util.remote";
import {
  appendNotice,
  appendOutput,
  emptyOutput,
  type OutputBuffer,
} from "@/lib/util.remote-output";

/** Paused, or the window hidden, this long: the follow ends until the person comes back. */
const IDLE_MS = 60_000;
/** A follow lost with its session is asked again this soon. */
const RETRY_MS = 3000;

export type OutputStatus =
  | { kind: "connecting" }
  | { kind: "following" }
  | { kind: "reconnecting" }
  /** Paused or hidden for a minute: nothing is followed. */
  | { kind: "idle" }
  | { kind: "ended"; interrupted: boolean }
  /** The run is not the service's last any more: the next snapshot names the new one. */
  | { kind: "gone" }
  | { kind: "failed"; code: string; message: string };

function isDone(status: OutputStatus): boolean {
  return ["ended", "gone", "failed"].includes(status.kind);
}

/**
 * Follows a run's output on another machine while the panel is open (docs/remote-machines.md,
 * "Following"). It starts at the run's tail, follows again from where it stopped after a lost
 * session, a pause or a hidden window, and moves to a new run from its start.
 */
export function useRemoteOutput(machineId: string, runId: string | null, paused: boolean) {
  const visible = useWindowVisible(IDLE_MS);
  const [pausedLong, setPausedLong] = useState(false);
  const [buffer, setBuffer] = useState<OutputBuffer>(emptyOutput);
  const [status, setStatus] = useState<OutputStatus>({ kind: "connecting" });
  const [attempt, setAttempt] = useState(0);
  const next = useRef<number | null>(null);
  const followedRun = useRef<string | null>(null);

  // A restart: the new run, from its start.
  useEffect(() => {
    if (!runId) return;
    if (followedRun.current && followedRun.current !== runId) {
      setBuffer((current) =>
        appendNotice(current, `Restarted at ${formatClock(Date.now() / 1000)}`),
      );
      next.current = 0;
      setStatus({ kind: "connecting" });
    }
    followedRun.current = runId;
  }, [runId]);

  useEffect(() => {
    if (!paused) {
      setPausedLong(false);
      return;
    }
    const timer = window.setTimeout(() => setPausedLong(true), IDLE_MS);
    return () => window.clearTimeout(timer);
  }, [paused]);

  const idle = pausedLong || !visible;
  const done = isDone(status);

  useEffect(() => {
    if (idle && !done) setStatus({ kind: "idle" });
  }, [idle, done]);

  useEffect(() => {
    if (!runId || idle || done) return;
    let active = true;
    let close: (() => void) | null = null;
    let timer: number | undefined;
    let finished = false;
    const retry = () => {
      if (!active) return;
      setStatus({ kind: "reconnecting" });
      timer = window.setTimeout(() => setAttempt((count) => count + 1), RETRY_MS);
    };
    const onEvent = (event: RemoteFollowEventDto) => {
      if (event.event === "data") {
        next.current = event.next;
        setBuffer((current) => appendOutput(current, event));
        if (event.ended) {
          finished = true;
          setStatus({ kind: "ended", interrupted: Boolean(event.interrupted) });
        } else {
          setStatus({ kind: "following" });
        }
        return;
      }
      finished = true;
      if (event.code === "gone") setStatus({ kind: "gone" });
      else if (isFinalCode(event.code) || event.code === "not_shared") {
        setStatus({ kind: "failed", code: event.code, message: event.message });
      } else retry();
    };
    setStatus((current) => (current.kind === "following" ? current : { kind: "connecting" }));
    openRemoteStream<RemoteFollowEventDto>(
      { kind: "follow", machineId, runId, offset: next.current },
      {
        onData: onEvent,
        // The daemon went away mid-follow.
        onEnd: () => {
          if (!finished) retry();
        },
      },
    )
      .then((closeStream) => {
        if (active) close = closeStream;
        else closeStream();
      })
      .catch(retry);
    return () => {
      active = false;
      window.clearTimeout(timer);
      close?.();
    };
  }, [machineId, runId, idle, done, attempt]);

  return { buffer, status };
}
