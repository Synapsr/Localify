// The relay between the machines of one account (docs/remote-machines.md,
// "Relay" and "Wire protocol"; ADR 0028). One object per account, named
// `user:<user id>`, holding one hibernatable WebSocket per machine. It
// routes opaque, end-to-end encrypted frames by machine id and reads only
// their envelope. It stores nothing: no storage call, no alarm, no timer, no
// history. Each socket's attachment holds who it is, and the routing map is
// rebuilt from the attachments after the object hibernates.

import { DurableObject } from "cloudflare:workers";

import { accountOf, REMOTE_CHECK_SQL, type RemoteCheckRow } from "./entitlement";

/** The WebSocket subprotocol: the relay protocol's version. */
export const RELAY_PROTOCOL = "localify-remote.1";

/**
 * What the Worker tells the object about a socket it checked. It removes
 * every header with this prefix from the client's request first.
 */
export const RELAY_HEADER_PREFIX = "x-localify-relay-";
export const RELAY_HEADERS = {
  user: `${RELAY_HEADER_PREFIX}user`,
  machine: `${RELAY_HEADER_PREFIX}machine`,
  version: `${RELAY_HEADER_PREFIX}version`,
  proto: `${RELAY_HEADER_PREFIX}proto`,
  /** When the Worker checked the token, the ban and remote access, in Unix ms. */
  checked: `${RELAY_HEADER_PREFIX}checked`,
} as const;

export const CLOSE = {
  tooLarge: 1009,
  badFrame: 4000,
  replaced: 4001,
  revoked: 4401,
  lapsed: 4402,
  banned: 4403,
  rateLimited: 4429,
} as const;

type AccountClose = typeof CLOSE.lapsed | typeof CLOSE.banned;
type Proto = { min: number; max: number };

export const MAX_FRAME_BYTES = 64 * 1024;
/** How long a socket's authorization holds before its next frame checks it again. */
export const AUTHORIZATION_MS = 60_000;
/** The wait an `unavailable` error asks for, in seconds. */
const UNAVAILABLE_RETRY_AFTER = 5;
/** One socket per active machine, and an account has at most 10. */
const MAX_SOCKETS = 10;
/** Per socket: 50 frames a second, bursts of 200; 512 KiB a second over 10 seconds. */
const FRAME_RATE = 50;
const FRAME_BURST = 200;
const BYTE_RATE = 512 * 1024;
const BYTE_BURST = 10 * BYTE_RATE;
/** Frames refused in a row before the socket is closed with 4429. */
const REFUSALS_BEFORE_CLOSE = 50;
/** The wait a 4429 close asks for, in seconds. */
const RATE_LIMITED_RETRY_AFTER = 10;
const OPEN = 1;

/** A socket's identity, in its attachment (at most 2 KiB). */
export type Attachment = {
  userId: string;
  machineId: string;
  connId: string;
  version: string;
  proto: Proto;
  /** Unix ms. */
  connectedAt: number;
  /** Unix ms; a frame after it checks the machine again. */
  authorizedUntil: number;
};

type Connection = { ws: WebSocket; attachment: Attachment };
type Verdict = "ok" | "unavailable" | typeof CLOSE.revoked | AccountClose;
type Budget = { frames: number; bytes: number; at: number; refused: number };

type Send = {
  to: string;
  sid: string;
  kind: "hs" | "msg" | "close";
  purpose?: "pair" | "session";
  data?: string;
};

const MACHINE_ID = /^[A-Za-z0-9_-]{1,64}$/;
const SESSION_ID = /^[A-Za-z0-9_-]{22}$/;
const BASE64URL = /^[A-Za-z0-9_-]+$/;

export class RemoteRelay extends DurableObject<Env> {
  /** Machine id to its socket, rebuilt from the attachments after hibernation. */
  private readonly sockets = new Map<string, Connection>();
  /** In memory only: they start over after hibernation, more permissive for a moment. */
  private readonly budgets = new Map<string, Budget>();
  /** Each connection's frames, handled one after the other so none overtakes another. */
  private readonly tails = new Map<string, Promise<void>>();
  /** The D1 check in flight for a machine, which concurrent frames share. */
  private readonly checks = new Map<string, Promise<Verdict>>();

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    // Answered by the runtime without waking the object.
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
    for (const ws of ctx.getWebSockets()) {
      const attachment = ws.deserializeAttachment() as Attachment | null;
      if (!attachment || ws.readyState !== OPEN) continue;
      const known = this.sockets.get(attachment.machineId);
      if (!known || known.attachment.connectedAt < attachment.connectedAt) {
        this.sockets.set(attachment.machineId, { ws, attachment });
      }
    }
  }

  /** The upgrade, after the Worker checked the token, the ban, remote access and the key. */
  async fetch(request: Request): Promise<Response> {
    const headers = request.headers;
    const userId = headers.get(RELAY_HEADERS.user);
    const machineId = headers.get(RELAY_HEADERS.machine);
    const checked = Number(headers.get(RELAY_HEADERS.checked));
    const proto = parseProto(headers.get(RELAY_HEADERS.proto));
    if (!userId || !machineId || !Number.isFinite(checked) || !proto) {
      return new Response("Not a relay connection", { status: 400 });
    }
    const now = Date.now();
    const attachment: Attachment = {
      userId,
      machineId,
      connId: crypto.randomUUID(),
      version: headers.get(RELAY_HEADERS.version) ?? "unknown",
      proto,
      connectedAt: now,
      authorizedUntil: Math.min(checked, now) + AUTHORIZATION_MS,
    };
    const { 0: client, 1: server } = new WebSocketPair();
    this.ctx.acceptWebSocket(server, [machineId]);
    server.serializeAttachment(attachment);

    const previous = this.sockets.get(machineId);
    this.sockets.set(machineId, { ws: server, attachment });
    if (previous) this.drop(previous, CLOSE.replaced, "replaced by a newer connection");
    // Only a machine signed in more than once at a time gets here.
    while (this.sockets.size > MAX_SOCKETS) {
      const oldest = [...this.sockets.values()].reduce((a, b) =>
        a.attachment.connectedAt <= b.attachment.connectedAt ? a : b,
      );
      this.drop(oldest, CLOSE.replaced, "too many connections", true);
    }

    const peers = [...this.sockets.values()]
      .filter((peer) => peer.attachment.machineId !== machineId)
      .map((peer) => presence(peer.attachment, "online", peer.attachment.connectedAt));
    send(server, { t: "welcome", v: 1, connId: attachment.connId, machineId, peers });
    this.broadcast(presence(attachment, "online", now), machineId);
    return new Response(null, {
      status: 101,
      webSocket: client,
      headers: { "sec-websocket-protocol": RELAY_PROTOCOL },
    });
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    const sender = this.connectionOf(ws);
    if (!sender) return;
    const connId = sender.attachment.connId;
    const tail = (this.tails.get(connId) ?? Promise.resolve())
      .then(() => this.handle(sender.attachment.machineId, connId, message))
      .catch((error: unknown) => console.error("The relay could not handle a frame", error));
    this.tails.set(connId, tail);
    await tail;
    if (this.tails.get(connId) === tail) this.tails.delete(connId);
  }

  async webSocketClose(ws: WebSocket): Promise<void> {
    this.closed(ws);
    // Completes the closing handshake; the socket may already be closed.
    closeQuietly(ws, 1000, "closed");
  }

  async webSocketError(ws: WebSocket): Promise<void> {
    this.closed(ws);
  }

  /**
   * The machine left the account: its socket closes with 4401, and every
   * other machine is told to delete its pin, whether or not it was online.
   */
  async revokeMachine(machineId: string): Promise<void> {
    const connection = this.sockets.get(machineId);
    if (connection) this.drop(connection, CLOSE.revoked, "machine removed from the account");
    for (const ws of this.ctx.getWebSockets(machineId)) {
      closeQuietly(ws, CLOSE.revoked, "machine removed from the account");
    }
    this.broadcast({ t: "revoked", machineId });
  }

  /** The account lost remote access (4402) or was banned (4403): every socket closes. */
  async revokeAccount(code: AccountClose): Promise<void> {
    if (code !== CLOSE.lapsed && code !== CLOSE.banned)
      throw new Error(`Not an account close: ${code}`);
    for (const connection of [...this.sockets.values()]) {
      this.drop(connection, code, closeReason(code));
    }
    for (const ws of this.ctx.getWebSockets()) closeQuietly(ws, code, closeReason(code));
  }

  // --- Frames ---

  private async handle(machineId: string, connId: string, message: string | ArrayBuffer) {
    const sender = this.sockets.get(machineId);
    if (!sender || sender.attachment.connId !== connId) return;
    const size = typeof message === "string" ? utf8Length(message) : message.byteLength;
    if (size > MAX_FRAME_BYTES) {
      this.drop(sender, CLOSE.tooLarge, "frame too large", true);
      return;
    }
    if (typeof message !== "string") {
      this.drop(sender, CLOSE.badFrame, "binary frame", true);
      return;
    }
    // The auto-response answers it while the object sleeps; this is the same answer awake.
    if (message === "ping") {
      send(sender.ws, "pong");
      return;
    }
    const parsed = parseSend(message);
    if (parsed === null) {
      this.drop(sender, CLOSE.badFrame, "not a send frame", true);
      return;
    }
    const sid = parsed.sid;
    const refused = this.spend(sender, size);
    if (refused?.close) {
      const reason = JSON.stringify({ retryAfter: RATE_LIMITED_RETRY_AFTER });
      this.drop(sender, CLOSE.rateLimited, reason, true);
      return;
    }
    if (refused) {
      sendError(
        sender.ws,
        "rate_limited",
        "Too many frames from this machine.",
        sid,
        refused.retryAfter,
      );
      return;
    }
    if ("error" in parsed) {
      sendError(sender.ws, "bad_frame", parsed.error, sid);
      return;
    }
    const frame = parsed;
    if (frame.to === machineId) {
      sendError(sender.ws, "bad_frame", "A frame cannot go to its own sender.", sid);
      return;
    }

    const recipient = this.sockets.get(frame.to);
    const now = Date.now();
    const due = [sender, recipient].filter(
      (connection): connection is Connection =>
        connection !== undefined && connection.attachment.authorizedUntil <= now,
    );
    if (due.length > 0) {
      const verdicts = await this.authorize(
        sender.attachment.userId,
        due.map((connection) => connection.attachment.machineId),
      );
      if (verdicts.includes("unavailable")) {
        if (this.isCurrent(sender)) {
          sendError(
            sender.ws,
            "unavailable",
            "Localify Cloud cannot check this frame right now.",
            sid,
            UNAVAILABLE_RETRY_AFTER,
          );
        }
        return;
      }
      const until = Date.now() + AUTHORIZATION_MS;
      due.forEach((connection, index) => {
        const verdict = verdicts[index]!;
        if (!this.isCurrent(connection)) return;
        if (verdict === "ok") {
          connection.attachment.authorizedUntil = until;
          connection.ws.serializeAttachment(connection.attachment);
        } else if (verdict !== "unavailable") {
          this.drop(connection, verdict, closeReason(verdict), true);
        }
      });
    }
    if (!this.isCurrent(sender)) return;
    const target = this.sockets.get(frame.to);
    if (!target) {
      sendError(sender.ws, "peer_offline", `${frame.to} is not connected.`, sid);
      return;
    }
    send(target.ws, {
      t: "recv",
      from: machineId,
      sid: frame.sid,
      kind: frame.kind,
      ...(frame.purpose ? { purpose: frame.purpose } : {}),
      ...(frame.kind !== "close" && frame.data !== undefined ? { data: frame.data } : {}),
    });
  }

  /**
   * The verdicts for these machines of the account, in order, from one D1
   * query for those no other frame is checking already. A D1 failure is
   * `unavailable` for all of them: the relay fails closed.
   */
  private async authorize(userId: string, machineIds: string[]): Promise<Verdict[]> {
    const missing = machineIds.filter((id) => !this.checks.has(id));
    if (missing.length > 0) {
      const query = this.check(userId, missing);
      for (const id of missing) {
        const verdict = query.then((verdicts) => verdicts.get(id) ?? CLOSE.revoked);
        this.checks.set(id, verdict);
        void verdict.then(() => {
          if (this.checks.get(id) === verdict) this.checks.delete(id);
        });
      }
    }
    return Promise.all(machineIds.map((id) => this.checks.get(id)!));
  }

  private async check(userId: string, machineIds: string[]): Promise<Map<string, Verdict>> {
    const all = (verdict: Verdict) => new Map(machineIds.map((id) => [id, verdict]));
    let row: RemoteCheckRow | null;
    try {
      row = await this.env.DB.prepare(REMOTE_CHECK_SQL)
        .bind(userId, JSON.stringify(machineIds))
        .first<RemoteCheckRow>();
    } catch (error) {
      console.error("The relay could not check a frame's machines in D1", error);
      return all("unavailable");
    }
    if (!row) return all(CLOSE.revoked);
    const active = new Set(JSON.parse(row.active_machines) as string[]);
    const account = accountOf(row, Date.now());
    return new Map(
      machineIds.map((id): [string, Verdict] => [
        id,
        !active.has(id)
          ? CLOSE.revoked
          : account.banned
            ? CLOSE.banned
            : !account.entitlement.remoteAccess
              ? CLOSE.lapsed
              : "ok",
      ]),
    );
  }

  /**
   * Takes the frame from the socket's budget, or says how many seconds to
   * wait, and whether the socket went on long enough to be closed.
   */
  private spend(
    connection: Connection,
    bytes: number,
  ): { retryAfter: number; close: boolean } | null {
    const now = Date.now();
    const connId = connection.attachment.connId;
    const budget = this.budgets.get(connId) ?? {
      frames: FRAME_BURST,
      bytes: BYTE_BURST,
      at: now,
      refused: 0,
    };
    this.budgets.set(connId, budget);
    const elapsed = Math.max(0, now - budget.at) / 1000;
    budget.frames = Math.min(FRAME_BURST, budget.frames + elapsed * FRAME_RATE);
    budget.bytes = Math.min(BYTE_BURST, budget.bytes + elapsed * BYTE_RATE);
    budget.at = now;
    if (budget.frames >= 1 && budget.bytes >= bytes) {
      budget.frames -= 1;
      budget.bytes -= bytes;
      budget.refused = 0;
      return null;
    }
    budget.refused += 1;
    const wait = Math.max((1 - budget.frames) / FRAME_RATE, (bytes - budget.bytes) / BYTE_RATE);
    return {
      retryAfter: Math.max(1, Math.ceil(wait)),
      close: budget.refused >= REFUSALS_BEFORE_CLOSE,
    };
  }

  // --- Connections ---

  private connectionOf(ws: WebSocket): Connection | null {
    const attachment = ws.deserializeAttachment() as Attachment | null;
    const connection = attachment ? this.sockets.get(attachment.machineId) : undefined;
    return connection && connection.attachment.connId === attachment?.connId ? connection : null;
  }

  private isCurrent(connection: Connection): boolean {
    return this.sockets.get(connection.attachment.machineId) === connection;
  }

  /** The socket closed on its own: the others see the machine go offline. */
  private closed(ws: WebSocket): void {
    const connection = this.connectionOf(ws);
    if (!connection) return;
    this.forget(connection);
    this.broadcast(presence(connection.attachment, "offline", Date.now()));
  }

  /**
   * Closes a socket the relay ends. When it was its machine's current one,
   * and `offline` says so, the others see the machine go offline.
   */
  private drop(connection: Connection, code: number, reason: string, offline = false): void {
    closeQuietly(connection.ws, code, reason);
    const current = this.isCurrent(connection);
    this.forget(connection);
    if (current && offline) this.broadcast(presence(connection.attachment, "offline", Date.now()));
  }

  private forget(connection: Connection): void {
    if (this.isCurrent(connection)) this.sockets.delete(connection.attachment.machineId);
    this.budgets.delete(connection.attachment.connId);
  }

  /** Sends to every connected machine but `except`. */
  private broadcast(message: unknown, except?: string): void {
    const text = JSON.stringify(message);
    for (const [machineId, connection] of this.sockets) {
      if (machineId !== except) send(connection.ws, text);
    }
  }
}

/** The relay object of an account. */
export function relayOf(env: Env, userId: string): DurableObjectStub<RemoteRelay> {
  return env.RELAY.get(env.RELAY.idFromName(`user:${userId}`));
}

/**
 * Pushes a revocation to the account's relay. A failure is logged and
 * otherwise ignored: the relay's lazy checks and the leases catch up within
 * a minute, and the change that caused it has already been written.
 */
export async function pushToRelay(
  env: Env,
  userId: string,
  push: (relay: DurableObjectStub<RemoteRelay>) => Promise<void>,
): Promise<void> {
  try {
    await push(relayOf(env, userId));
  } catch (error) {
    console.error("A push to the relay failed; its lazy checks catch up within a minute", error);
  }
}

/** `<min>-<max>`, the session protocol versions a daemon speaks; 1 to 1 when absent. */
export function parseProto(value: string | null): Proto | null {
  if (value === null) return { min: 1, max: 1 };
  const match = /^([1-9]\d{0,2})-([1-9]\d{0,2})$/.exec(value);
  if (!match) return null;
  const min = Number(match[1]);
  const max = Number(match[2]);
  return min <= max ? { min, max } : null;
}

/**
 * A `send` frame, or what is wrong with one whose envelope parsed, with its
 * session id when it has a valid one; null for anything that is not a JSON
 * `send` frame at all. Unknown fields are ignored.
 */
function parseSend(text: string): Send | { error: string; sid?: string } | null {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const frame = value as Record<string, unknown>;
  if (frame.t !== "send") return null;
  const sid = typeof frame.sid === "string" && SESSION_ID.test(frame.sid) ? frame.sid : undefined;
  const bad = (error: string) => (sid === undefined ? { error } : { error, sid });
  if (sid === undefined) return bad("sid must be a session id: 22 base64url characters.");
  if (typeof frame.to !== "string" || !MACHINE_ID.test(frame.to)) {
    return bad("to must be a machine id.");
  }
  if (frame.kind !== "hs" && frame.kind !== "msg" && frame.kind !== "close") {
    return bad('kind must be "hs", "msg" or "close".');
  }
  if (frame.purpose !== undefined && frame.purpose !== "pair" && frame.purpose !== "session") {
    return bad('purpose must be "pair" or "session".');
  }
  if (frame.kind !== "close" && (typeof frame.data !== "string" || !BASE64URL.test(frame.data))) {
    return bad("data must be a base64url Noise message.");
  }
  return {
    to: frame.to,
    sid,
    kind: frame.kind,
    ...(frame.purpose !== undefined ? { purpose: frame.purpose } : {}),
    ...(typeof frame.data === "string" ? { data: frame.data } : {}),
  };
}

function presence(attachment: Attachment, state: "online" | "offline", since: number) {
  return {
    t: "presence",
    machineId: attachment.machineId,
    state,
    version: attachment.version,
    proto: attachment.proto,
    since: new Date(since).toISOString(),
  };
}

function sendError(
  ws: WebSocket,
  code: "peer_offline" | "rate_limited" | "bad_frame" | "unavailable",
  message: string,
  sid?: string,
  retryAfter?: number,
): void {
  send(ws, {
    t: "error",
    code,
    ...(sid !== undefined ? { sid } : {}),
    message,
    ...(retryAfter !== undefined ? { retryAfter } : {}),
  });
}

function closeReason(code: Verdict): string {
  switch (code) {
    case CLOSE.revoked:
      return "token revoked";
    case CLOSE.lapsed:
      return "remote access lapsed";
    case CLOSE.banned:
      return "account suspended";
    default:
      return "closed";
  }
}

/** A socket that closed meanwhile drops what is sent to it. */
function send(ws: WebSocket, message: unknown): void {
  try {
    ws.send(typeof message === "string" ? message : JSON.stringify(message));
  } catch {
    // Closed meanwhile.
  }
}

function closeQuietly(ws: WebSocket, code: number, reason: string): void {
  try {
    ws.close(code, reason);
  } catch {
    // Closed already.
  }
}

/** A string's UTF-8 length; one longer than the cap in UTF-16 units is longer in bytes too. */
function utf8Length(text: string): number {
  return text.length > MAX_FRAME_BYTES ? text.length : new TextEncoder().encode(text).byteLength;
}
