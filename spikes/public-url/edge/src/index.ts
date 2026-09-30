import { DurableObject } from "cloudflare:workers";
import {
  FrameType,
  chunks,
  decodeFrame,
  decodeJson,
  decodeText,
  encodeFrame,
  encodeJson,
  encodeText,
  type Frame,
  type RequestHead,
  type ResponseHead,
  type WsClose,
} from "./protocol";

interface Env {
  PUBLIC_ENDPOINT: DurableObjectNamespace<PublicEndpoint>;
  PUBLIC_BASE_DOMAIN: string;
  CONNECTOR_TOKEN: string;
  /** Spike-only: origin fetched directly by the baseline labels. */
  SPIKE_BASELINE_ORIGIN?: string;
}

// Spike-only measurement labels. They skip parts of the path so the bench can
// separate local runtime overhead from the connector protocol.
const BASELINE_WORKER_LABEL = "baseline-worker";
const BASELINE_DO_LABEL = "baseline-do";

const CONNECT_PATH = "/_localify/connect";
const PUBLIC_HOST_HEADER = "x-localify-public-host";
const LABEL_HEADER = "x-localify-label";
const CONNECTOR_TAG = "connector";
const RESPONSE_HEAD_TIMEOUT_MS = 60_000;
const WEBSOCKET_HEAD_TIMEOUT_MS = 15_000;

const HOP_BY_HOP = new Set([
  "connection",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "proxy-connection",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
]);
// The connector performs its own WebSocket handshake with the upstream app.
const WEBSOCKET_HANDSHAKE = new Set([
  "sec-websocket-key",
  "sec-websocket-version",
  "sec-websocket-extensions",
  "sec-websocket-accept",
]);
const NULL_BODY_STATUSES = new Set([101, 204, 205, 304]);

/**
 * Splits `<label>--<slug>.<base>` or `<slug>.<base>` (ADR 0015). The last `--`
 * separates the wildcard label from the slug.
 */
export function parsePublicHost(
  host: string,
  baseDomain: string,
): { slug: string; label: string | null } | null {
  const suffix = `.${baseDomain}`;
  if (!host.endsWith(suffix)) return null;
  const name = host.slice(0, -suffix.length);
  if (!name || name.includes(".")) return null;
  const separator = name.lastIndexOf("--");
  if (separator === -1) return { slug: name, label: null };
  const label = name.slice(0, separator);
  const slug = name.slice(separator + 2);
  return label && slug ? { slug, label } : null;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const host = (request.headers.get("host") ?? url.host).split(":")[0].toLowerCase();
    const target = parsePublicHost(host, env.PUBLIC_BASE_DOMAIN);

    if (!target) {
      if (url.pathname !== CONNECT_PATH) {
        return new Response("Unknown public URL", { status: 404 });
      }
      if (request.headers.get("authorization") !== `Bearer ${env.CONNECTOR_TOKEN}`) {
        return new Response("Unauthorized", { status: 401 });
      }
      const slug = url.searchParams.get("slug");
      if (!slug) return new Response("Missing slug", { status: 400 });
      return endpoint(env, slug).fetch(request);
    }

    if (target.label === BASELINE_WORKER_LABEL && env.SPIKE_BASELINE_ORIGIN) {
      return fetchBaseline(request, env.SPIKE_BASELINE_ORIGIN);
    }

    // Overwrite, never trust, client-supplied Localify headers.
    const headers = new Headers(request.headers);
    headers.set(PUBLIC_HOST_HEADER, host);
    headers.set(LABEL_HEADER, target.label ?? "");
    return endpoint(env, target.slug).fetch(new Request(request, { headers }));
  },
} satisfies ExportedHandler<Env>;

function endpoint(env: Env, slug: string): DurableObjectStub<PublicEndpoint> {
  return env.PUBLIC_ENDPOINT.get(env.PUBLIC_ENDPOINT.idFromName(slug));
}

function fetchBaseline(request: Request, origin: string): Promise<Response> {
  const url = new URL(request.url);
  return fetch(new Request(origin + url.pathname + url.search, request));
}

interface OpenStream {
  websocket: boolean;
  resolveHead: (head: ResponseHead) => void;
  rejectHead: (error: Error) => void;
  /** HTTP response body; absent for WebSocket streams. */
  body?: ReadableStreamDefaultController<Uint8Array>;
  /** WebSocket frames that arrive before the browser socket is accepted. */
  pendingWsFrames: Frame[];
}

/**
 * One public endpoint: owns the connector session and multiplexes public
 * requests over it (ADR 0003, ADR 0005).
 */
export class PublicEndpoint extends DurableObject<Env> {
  private streams = new Map<number, OpenStream>();
  private nextStreamId = 1;

  async fetch(request: Request): Promise<Response> {
    if (!request.headers.has(PUBLIC_HOST_HEADER)) {
      return this.acceptConnector(request);
    }
    if (request.headers.get(LABEL_HEADER) === BASELINE_DO_LABEL && this.env.SPIKE_BASELINE_ORIGIN) {
      return fetchBaseline(request, this.env.SPIKE_BASELINE_ORIGIN);
    }
    return this.proxy(request);
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    const tags = this.ctx.getTags(ws);
    if (tags.includes(CONNECTOR_TAG)) {
      if (typeof message === "string") return;
      const frame = decodeFrame(message);
      if (frame) this.handleConnectorFrame(frame);
      return;
    }

    const streamId = streamIdFromTags(tags);
    const connector = this.connector();
    if (streamId === null) return;
    if (!connector) {
      safeClose(ws, 1011, "Endpoint offline");
      return;
    }
    if (typeof message === "string") {
      connector.send(encodeFrame(FrameType.WsText, streamId, encodeText(message)));
    } else {
      connector.send(encodeFrame(FrameType.WsBinary, streamId, new Uint8Array(message)));
    }
  }

  async webSocketClose(ws: WebSocket, code: number, reason: string): Promise<void> {
    this.handleSocketGone(ws, code, reason);
  }

  async webSocketError(ws: WebSocket): Promise<void> {
    this.handleSocketGone(ws, 1011, "WebSocket error");
  }

  private acceptConnector(request: Request): Response {
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      return new Response("Expected a WebSocket upgrade", { status: 426 });
    }
    // A new session replaces the previous one (explicit claim, ADR 0003).
    for (const previous of this.ctx.getWebSockets(CONNECTOR_TAG)) {
      safeClose(previous, 1012, "Replaced by a newer connector session");
    }
    this.failAllStreams("Connector session replaced");

    const { 0: client, 1: server } = new WebSocketPair();
    this.ctx.acceptWebSocket(server, [CONNECTOR_TAG]);
    return new Response(null, { status: 101, webSocket: client });
  }

  private async proxy(request: Request): Promise<Response> {
    const connector = this.connector();
    if (!connector) {
      return new Response("This public URL is offline: no Localify connector is serving it.", {
        status: 502,
      });
    }

    const url = new URL(request.url);
    const websocket = request.headers.get("upgrade")?.toLowerCase() === "websocket";
    const streamId = this.allocateStreamId();
    const head: RequestHead = {
      method: request.method,
      path: url.pathname + url.search,
      headers: forwardableHeaders(request.headers),
      publicHost: request.headers.get(PUBLIC_HOST_HEADER) ?? url.hostname,
      label: request.headers.get(LABEL_HEADER) || null,
      hasBody: !websocket && request.body !== null,
      websocket,
    };

    const { headPromise, body } = this.openStream(streamId, websocket);
    connector.send(encodeFrame(FrameType.RequestHead, streamId, encodeJson(head)));

    if (websocket) {
      return this.completeWebSocket(streamId, headPromise);
    }

    if (request.body) {
      this.ctx.waitUntil(this.pumpRequestBody(connector, streamId, request.body));
    }

    let responseHead: ResponseHead;
    try {
      responseHead = await withTimeout(headPromise, RESPONSE_HEAD_TIMEOUT_MS);
    } catch (error) {
      this.resetStream(streamId, errorMessage(error));
      return new Response(`Public URL error: ${errorMessage(error)}`, { status: 504 });
    }

    const headers = new Headers();
    for (const [name, value] of responseHead.headers) {
      if (!HOP_BY_HOP.has(name.toLowerCase())) headers.append(name, value);
    }
    const nullBody = request.method === "HEAD" || NULL_BODY_STATUSES.has(responseHead.status);
    return new Response(nullBody ? null : body, {
      status: responseHead.status,
      headers,
      // The upstream body is already encoded; do not let the runtime re-encode it.
      encodeBody: headers.has("content-encoding") ? "manual" : "automatic",
    });
  }

  private openStream(
    streamId: number,
    websocket: boolean,
  ): { headPromise: Promise<ResponseHead>; body: ReadableStream<Uint8Array> | null } {
    let resolveHead!: (head: ResponseHead) => void;
    let rejectHead!: (error: Error) => void;
    const headPromise = new Promise<ResponseHead>((resolve, reject) => {
      resolveHead = resolve;
      rejectHead = reject;
    });
    const stream: OpenStream = { websocket, resolveHead, rejectHead, pendingWsFrames: [] };
    this.streams.set(streamId, stream);

    const body = websocket
      ? null
      : new ReadableStream<Uint8Array>({
          start: (controller) => {
            stream.body = controller;
          },
          cancel: () => {
            // The browser went away; stop the upstream request.
            this.resetStream(streamId, "Client closed the response");
          },
        });
    return { headPromise, body };
  }

  private async pumpRequestBody(
    connector: WebSocket,
    streamId: number,
    body: ReadableStream<Uint8Array>,
  ): Promise<void> {
    const reader = body.getReader();
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        if (!this.streams.has(streamId)) {
          await reader.cancel();
          return;
        }
        for (const chunk of chunks(value)) {
          connector.send(encodeFrame(FrameType.RequestBody, streamId, chunk));
        }
      }
      connector.send(encodeFrame(FrameType.RequestEnd, streamId));
    } catch (error) {
      this.resetStream(streamId, `Request body failed: ${errorMessage(error)}`);
    }
  }

  private async completeWebSocket(
    streamId: number,
    headPromise: Promise<ResponseHead>,
  ): Promise<Response> {
    let head: ResponseHead;
    try {
      head = await withTimeout(headPromise, WEBSOCKET_HEAD_TIMEOUT_MS);
    } catch (error) {
      this.resetStream(streamId, errorMessage(error));
      return new Response(`Public URL error: ${errorMessage(error)}`, { status: 504 });
    }

    const stream = this.streams.get(streamId);
    this.streams.delete(streamId);
    if (head.status !== 101) {
      return new Response(`The local app refused the WebSocket (${head.status}).`, {
        status: 502,
      });
    }

    const { 0: client, 1: server } = new WebSocketPair();
    this.ctx.acceptWebSocket(server, [streamTag(streamId)]);
    for (const frame of stream?.pendingWsFrames ?? []) {
      deliverToBrowser(server, frame);
    }

    const headers = new Headers();
    const protocol = headerValue(head.headers, "sec-websocket-protocol");
    if (protocol) headers.set("sec-websocket-protocol", protocol);
    return new Response(null, { status: 101, webSocket: client, headers });
  }

  private handleConnectorFrame(frame: Frame): void {
    const stream = this.streams.get(frame.streamId);
    switch (frame.type) {
      case FrameType.ResponseHead:
        stream?.resolveHead(decodeJson<ResponseHead>(frame.payload));
        return;
      case FrameType.ResponseBody:
        try {
          // Copy: the payload is a view into a message buffer we do not own.
          stream?.body?.enqueue(frame.payload.slice());
        } catch {
          this.resetStream(frame.streamId, "Client closed the response");
        }
        return;
      case FrameType.ResponseEnd:
        try {
          stream?.body?.close();
        } catch {
          // Already closed or cancelled by the client.
        }
        if (!stream?.websocket) this.streams.delete(frame.streamId);
        return;
      case FrameType.Reset:
        this.failStream(frame.streamId, decodeText(frame.payload) || "Stream reset");
        return;
      case FrameType.WsText:
      case FrameType.WsBinary:
      case FrameType.WsClose: {
        const socket = this.browserSocket(frame.streamId);
        if (socket) {
          deliverToBrowser(socket, frame);
        } else if (stream?.websocket) {
          stream.pendingWsFrames.push({ ...frame, payload: frame.payload.slice() });
        }
        return;
      }
    }
  }

  private handleSocketGone(ws: WebSocket, code: number, reason: string): void {
    const tags = this.ctx.getTags(ws);
    if (tags.includes(CONNECTOR_TAG)) {
      // Ignore the close of a session that a newer connector already replaced.
      if (this.connector()) return;
      this.failAllStreams("Connector disconnected");
      return;
    }
    const streamId = streamIdFromTags(tags);
    if (streamId === null) return;
    const payload: WsClose = { code: sendableCloseCode(code), reason };
    this.connector()?.send(encodeFrame(FrameType.WsClose, streamId, encodeJson(payload)));
  }

  /** Stops a stream from our side and tells the connector. */
  private resetStream(streamId: number, reason: string): void {
    if (!this.streams.has(streamId) && !this.browserSocket(streamId)) return;
    this.failStream(streamId, reason);
    try {
      this.connector()?.send(encodeFrame(FrameType.Reset, streamId, encodeText(reason)));
    } catch {
      // Connector already gone.
    }
  }

  private failStream(streamId: number, reason: string): void {
    const stream = this.streams.get(streamId);
    this.streams.delete(streamId);
    if (stream) {
      const error = new Error(reason);
      stream.rejectHead(error);
      try {
        stream.body?.error(error);
      } catch {
        // Already closed.
      }
    }
    const socket = this.browserSocket(streamId);
    if (socket) safeClose(socket, 1011, reason);
  }

  private failAllStreams(reason: string): void {
    for (const streamId of [...this.streams.keys()]) this.failStream(streamId, reason);
    for (const socket of this.ctx.getWebSockets()) {
      if (!this.ctx.getTags(socket).includes(CONNECTOR_TAG)) {
        safeClose(socket, 1012, reason);
      }
    }
  }

  private connector(): WebSocket | undefined {
    return this.ctx
      .getWebSockets(CONNECTOR_TAG)
      .find((socket) => socket.readyState === WebSocket.OPEN);
  }

  private browserSocket(streamId: number): WebSocket | undefined {
    return this.ctx.getWebSockets(streamTag(streamId))[0];
  }

  private allocateStreamId(): number {
    // The counter resets when the object hibernates, so check live sockets too.
    for (;;) {
      const streamId = this.nextStreamId;
      this.nextStreamId = streamId >= 0xffff_ffff ? 1 : streamId + 1;
      if (!this.streams.has(streamId) && !this.browserSocket(streamId)) return streamId;
    }
  }
}

function forwardableHeaders(headers: Headers): [string, string][] {
  const forwarded: [string, string][] = [];
  for (const [name, value] of headers) {
    if (
      name === "host" ||
      name.startsWith("x-localify-") ||
      HOP_BY_HOP.has(name) ||
      WEBSOCKET_HANDSHAKE.has(name)
    ) {
      continue;
    }
    forwarded.push([name, value]);
  }
  return forwarded;
}

function deliverToBrowser(socket: WebSocket, frame: Frame): void {
  if (frame.type === FrameType.WsText) {
    socket.send(decodeText(frame.payload));
  } else if (frame.type === FrameType.WsBinary) {
    socket.send(frame.payload);
  } else if (frame.type === FrameType.WsClose) {
    const close = decodeJson<WsClose>(frame.payload);
    safeClose(socket, sendableCloseCode(close.code), close.reason);
  }
}

function streamTag(streamId: number): string {
  return `stream:${streamId}`;
}

function streamIdFromTags(tags: string[]): number | null {
  const tag = tags.find((value) => value.startsWith("stream:"));
  return tag ? Number(tag.slice("stream:".length)) : null;
}

function headerValue(headers: [string, string][], name: string): string | undefined {
  return headers.find(([key]) => key.toLowerCase() === name)?.[1];
}

/** 1005 and 1006 are reserved and cannot be sent in a close frame. */
function sendableCloseCode(code: number): number {
  return code === 1005 || code === 1006 ? 1000 : code;
}

function safeClose(socket: WebSocket, code: number, reason: string): void {
  try {
    // Close reasons are limited to 123 bytes.
    socket.close(code, reason.slice(0, 120));
  } catch {
    // Already closing.
  }
}

function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`No response within ${ms / 1000}s`)), ms);
  });
  return Promise.race([promise, timeout]).finally(() => {
    if (timer !== undefined) clearTimeout(timer);
  });
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
