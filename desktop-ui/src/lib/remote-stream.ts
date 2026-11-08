import { listen } from "@tauri-apps/api/event";

import type { RemoteStreamPayload, RemoteStreamRequest } from "../types/localify";

import { tauriClient } from "./tauri-client";

/** Emitted by `src-tauri/src/remote.rs`, one per line of a stream. */
const REMOTE_STREAM_EVENT = "remote-stream";
/** Lines kept for a stream whose id has not come back yet. */
const EARLY_LIMIT = 256;

type Handler = (payload: RemoteStreamPayload) => void;

const handlers = new Map<number, Handler>();
/** Lines that arrived before `openRemoteStream` learned their stream's id. */
const early = new Map<number, RemoteStreamPayload[]>();
let listening: Promise<unknown> | null = null;

function ensureListening() {
  listening ??= listen<RemoteStreamPayload>(REMOTE_STREAM_EVENT, (event) => {
    const payload = event.payload;
    const handler = handlers.get(payload.streamId);
    if (handler) {
      handler(payload);
      return;
    }
    const waiting = early.get(payload.streamId) ?? [];
    if (waiting.length < EARLY_LIMIT) waiting.push(payload);
    early.set(payload.streamId, waiting);
  });
  return listening;
}

export interface RemoteStreamHandlers<T> {
  onData: (data: T) => void;
  /** The daemon ended the stream, with the error that ended it if any. */
  onEnd: (error: string | null) => void;
}

/**
 * Opens a streaming management command through the desktop backend. Returns the function that
 * closes it, which ends what it watched or followed; `onEnd` is not called after it.
 */
export async function openRemoteStream<T>(
  request: RemoteStreamRequest,
  { onData, onEnd }: RemoteStreamHandlers<T>,
): Promise<() => void> {
  await ensureListening();
  const id = await tauriClient.openRemoteStream(request);
  let open = true;
  const handle = (payload: RemoteStreamPayload) => {
    if (!open) return;
    if (payload.done) {
      open = false;
      handlers.delete(id);
      onEnd(payload.error);
      return;
    }
    onData(payload.data as T);
  };
  handlers.set(id, handle);
  const waiting = early.get(id) ?? [];
  early.delete(id);
  waiting.forEach(handle);
  return () => {
    if (!open) return;
    open = false;
    handlers.delete(id);
    void tauriClient.closeRemoteStream(id).catch(() => undefined);
  };
}
