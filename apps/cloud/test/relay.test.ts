/// <reference types="node" />
// Remote machines (docs/remote-machines.md, "Relay" and "Cloud"): the roster,
// keys, removal, leases, the connect route and its pre-auth guard, and the
// relay object over the hibernatable sockets the tests hold.

import { randomBytes } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { sha256Hex } from "../src/crypto";
import { REMOTE_CHECK_SQL, REMOTE_REVOKE_SQL } from "../src/entitlement";
import {
  AUTHORIZATION_MS,
  CLOSE,
  MAX_FRAME_BYTES,
  MAX_PENDING_FRAMES,
  RELAY_PROTOCOL,
  type Attachment,
} from "../src/relay";
import {
  api,
  applyMigrations,
  createContext,
  signIn,
  type FakeSocket,
  type TestContext,
} from "./support";

const START = Date.parse("2026-11-08T10:00:00Z");
let context: TestContext;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["Date", "setTimeout", "setInterval"] });
  vi.setSystemTime(START);
  context = createContext();
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

type Machine = { token: string; machineId: string; userId: string; key: string };

const newKey = () => randomBytes(32).toString("base64url");
const sid = (n = 1) => Buffer.alloc(16, n).toString("base64url");
const iso = (ms: number) => new Date(ms).toISOString();
const UPGRADE = {
  upgrade: "websocket",
  "sec-websocket-protocol": RELAY_PROTOCOL,
  "x-localify-version": "0.30.0",
};

async function putKey(token: string, publicKey: unknown): Promise<Response> {
  return api(context, "PUT", "/v1/machines/self/remote-key", { token, body: { publicKey } });
}

async function errorCode(response: Response): Promise<string> {
  return ((await response.json()) as { error: string }).error;
}

/** A signed-in machine with its remote key uploaded; the clock moves on a millisecond. */
async function linked(login = "teo-goulois", githubId = 1, name = "laptop"): Promise<Machine> {
  const machine = await signIn(context, login, githubId, name);
  const key = newKey();
  expect((await putKey(machine.token, key)).status).toBe(204);
  vi.setSystemTime(Date.now() + 1);
  return { ...machine, key };
}

async function connect(token: string | null, headers: Record<string, string> = {}) {
  const response = await api(context, "GET", "/v1/remote/connect", {
    token: token ?? undefined,
    headers: { ...UPGRADE, ...headers },
  });
  const client = (response as unknown as { webSocket: FakeSocket | null }).webSocket;
  return { response, socket: client?.peer ?? null };
}

async function online(machine: { token: string }): Promise<FakeSocket> {
  const { response, socket } = await connect(machine.token);
  expect(response.status).toBe(101);
  return socket!;
}

/** The daemon behind `socket` sends `message`. */
async function deliver(
  machine: { userId: string },
  socket: FakeSocket,
  message: unknown,
): Promise<void> {
  await context.relays
    .object(machine.userId)
    .webSocketMessage(
      socket as unknown as WebSocket,
      typeof message === "string" ? message : JSON.stringify(message),
    );
}

function hs(to: string, extra: Record<string, unknown> = {}) {
  return { t: "send", to, sid: sid(), kind: "hs", purpose: "pair", data: "aGVsbG8", ...extra };
}

function msg(to: string) {
  return { t: "send", to, sid: sid(), kind: "msg", data: "AAAA" };
}

/** The machine ids of each relay check from now on, in order. */
function recordChecks(): string[][] {
  const checks: string[][] = [];
  const db = context.env.DB;
  (context.env as { DB: D1Database }).DB = {
    prepare: (sql: string) => {
      const statement = db.prepare(sql);
      if (sql !== REMOTE_CHECK_SQL) return statement;
      return {
        bind: (...params: unknown[]) => {
          checks.push(JSON.parse(String(params[1])) as string[]);
          return statement.bind(...params);
        },
      };
    },
    batch: (statements: D1PreparedStatement[]) => db.batch(statements),
  } as unknown as D1Database;
  return checks;
}

function unlimited(): void {
  context.remoteIpLimiter.limit = Number.POSITIVE_INFINITY;
  context.remoteConnectLimiter.limit = Number.POSITIVE_INFINITY;
}

function sql(text: string, ...params: (string | number | null)[]) {
  return context.sqlite.prepare(text).run(...params);
}

describe("the roster", () => {
  it("lists the account's active machines with their keys, this one marked, without an entitlement", async () => {
    const laptop = await signIn(context, "teo-goulois", 1, "laptop");
    vi.setSystemTime(Date.now() + 1);
    const runner = await linked("teo-goulois", 1, "runner");
    await linked("someone", 2, "desk");
    sql("DELETE FROM plan_overrides");

    const response = await api(context, "GET", "/v1/machines", { token: laptop.token });
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({
      machines: [
        {
          id: laptop.machineId,
          name: "laptop",
          createdAt: iso(START),
          remoteKey: null,
          self: true,
        },
        {
          id: runner.machineId,
          name: "runner",
          createdAt: iso(START + 1),
          remoteKey: runner.key,
          self: false,
        },
      ],
    });

    await api(context, "POST", "/v1/auth/logout", { token: runner.token });
    const after = (await (
      await api(context, "GET", "/v1/machines", { token: laptop.token })
    ).json()) as {
      machines: { id: string }[];
    };
    expect(after.machines.map((machine) => machine.id)).toEqual([laptop.machineId]);
  });
});

describe("remote keys", () => {
  it("are set once per machine id, and a same-name sign-in starts without one", async () => {
    const laptop = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const key = newKey();
    expect((await putKey(laptop.token, key)).status).toBe(204);
    expect((await putKey(laptop.token, key)).status).toBe(204);
    const other = await putKey(laptop.token, newKey());
    expect(other.status).toBe(409);
    expect(await errorCode(other)).toBe("remote_key_set");

    const runner = await signIn(context, "teo-goulois", 1, "runner");
    const taken = await putKey(runner.token, key);
    expect(taken.status).toBe(409);
    expect(await errorCode(taken)).toBe("remote_key_taken");

    // A new id, no key, and the old key stays the old machine's.
    const again = await signIn(context, "teo-goulois", 1, "laptop");
    expect(again.machineId).not.toBe(laptop.machineId);
    const roster = (await (
      await api(context, "GET", "/v1/machines", { token: again.token })
    ).json()) as {
      machines: { id: string; remoteKey: string | null }[];
    };
    expect(roster.machines.find((machine) => machine.id === again.machineId)?.remoteKey).toBeNull();
    expect(await errorCode(await putKey(again.token, key))).toBe("remote_key_taken");
    expect((await putKey(again.token, newKey())).status).toBe(204);
  });

  it("must be 32 bytes of canonical base64url", async () => {
    const { token } = await signIn(context);
    const key = newKey();
    for (const bad of [
      "",
      `${key}=`,
      key.slice(1),
      `${key.slice(0, 42)}B`,
      Buffer.alloc(32).toString("base64"),
      "A".repeat(44),
      42,
      null,
    ]) {
      const response = await putKey(token, bad);
      expect(response.status, String(bad)).toBe(422);
      expect(await errorCode(response)).toBe("invalid_key");
    }
  });

  it("are held by at most 10 machines of an account", async () => {
    const machines: Machine[] = [];
    for (let i = 0; i < 10; i++) machines.push(await linked("teo-goulois", 1, `machine-${i}`));
    // An eleventh active machine, which the sign-in limit never lets through.
    const { userId } = machines[0]!;
    const token = `lfy_${randomBytes(32).toString("hex")}`;
    sql(
      `INSERT INTO machines (id, user_id, name, status, ingress_port, created_at)
       VALUES ('extra', ?, 'extra', 'active', 7381, 1)`,
      userId,
    );
    sql(
      `INSERT INTO tokens (hash, user_id, machine_id, created_at) VALUES (?, ?, 'extra', 1)`,
      await sha256Hex(token),
      userId,
    );
    const refused = await putKey(token, newKey());
    expect(refused.status).toBe(403);
    expect(await errorCode(refused)).toBe("quota_exceeded");
  });
});

describe("removing a machine", () => {
  it("signs it out, closes its socket and tells the others to drop its pin", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const desk = await linked("teo-goulois", 1, "desk");
    const [sl, sr, sd] = [await online(laptop), await online(runner), await online(desk)];

    const response = await api(context, "DELETE", `/v1/machines/${runner.machineId}`, {
      token: laptop.token,
    });
    expect(response.status).toBe(204);
    expect(sr.closed).toEqual({ code: CLOSE.revoked, reason: expect.any(String) });
    for (const peer of [sl, sd]) {
      expect(peer.framesOf("revoked")).toEqual([{ t: "revoked", machineId: runner.machineId }]);
    }
    expect((await api(context, "GET", "/v1/me", { token: runner.token })).status).toBe(401);
    expect(
      context.sqlite.prepare("SELECT status FROM machines WHERE id = ?").get(runner.machineId),
    ).toEqual({ status: "revoked" });

    // Removing it again pushes again, for a push that failed before.
    expect(
      (await api(context, "DELETE", `/v1/machines/${runner.machineId}`, { token: laptop.token }))
        .status,
    ).toBe(204);
    expect(sl.framesOf("revoked")).toHaveLength(2);
  });

  it("works without the entitlement and banned, and on the caller itself is a sign-out", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    sql("DELETE FROM plan_overrides");
    sql("UPDATE users SET banned_at = 1");
    expect(
      (await api(context, "DELETE", `/v1/machines/${runner.machineId}`, { token: laptop.token }))
        .status,
    ).toBe(204);
    expect(
      (await api(context, "DELETE", `/v1/machines/${laptop.machineId}`, { token: laptop.token }))
        .status,
    ).toBe(204);
    expect((await api(context, "GET", "/v1/machines", { token: laptop.token })).status).toBe(401);
  });

  it("answers 404 for an unknown id and for another account's machine", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const desk = await linked("someone", 2, "desk");
    for (const id of ["nope", desk.machineId]) {
      const response = await api(context, "DELETE", `/v1/machines/${id}`, { token: laptop.token });
      expect(response.status).toBe(404);
      expect(await errorCode(response)).toBe("not_found");
    }
    expect((await api(context, "GET", "/v1/me", { token: desk.token })).status).toBe(200);
  });

  it("is pushed by a sign-out and by a same-name sign-in that replaces a machine", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const desk = await linked("teo-goulois", 1, "desk");
    const [sl, sr, sd] = [await online(laptop), await online(runner), await online(desk)];

    expect((await api(context, "POST", "/v1/auth/logout", { token: runner.token })).status).toBe(
      204,
    );
    expect(sr.closed?.code).toBe(CLOSE.revoked);
    expect(sl.framesOf("revoked")).toEqual([{ t: "revoked", machineId: runner.machineId }]);

    await signIn(context, "teo-goulois", 1, "laptop");
    expect(sl.closed?.code).toBe(CLOSE.revoked);
    expect(sd.framesOf("revoked")).toEqual([
      { t: "revoked", machineId: runner.machineId },
      { t: "revoked", machineId: laptop.machineId },
    ]);
  });

  it("keeps a sign-out that the relay push failed", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const laptop = await linked();
    (context.env as { RELAY: unknown }).RELAY = {
      idFromName: (name: string) => ({ name }),
      get: () => ({
        revokeMachine: async () => {
          throw new Error("relay down");
        },
      }),
    };
    expect((await api(context, "POST", "/v1/auth/logout", { token: laptop.token })).status).toBe(
      204,
    );
    expect((await api(context, "GET", "/v1/me", { token: laptop.token })).status).toBe(401);
    expect(error).toHaveBeenCalled();
  });
});

describe("leases", () => {
  it("give a lifetime to count from the request and the account's active machines, no time", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    await linked("someone", 2, "desk");
    const response = await api(context, "GET", "/v1/remote/lease", { token: laptop.token });
    expect(response.status).toBe(200);
    expect(response.headers.get("cache-control")).toBe("no-store");
    expect(await response.json()).toEqual({
      ttl: 60,
      machines: [laptop.machineId, runner.machineId],
    });

    await api(context, "DELETE", `/v1/machines/${runner.machineId}`, { token: laptop.token });
    expect(
      await (await api(context, "GET", "/v1/remote/lease", { token: laptop.token })).json(),
    ).toEqual({
      ttl: 60,
      machines: [laptop.machineId],
    });
    expect((await api(context, "GET", "/v1/remote/lease", { token: runner.token })).status).toBe(
      401,
    );
  });

  it("are refused without remote access, and to a banned account", async () => {
    const laptop = await linked();
    sql("UPDATE plan_overrides SET remote_access = 0");
    const lapsed = await api(context, "GET", "/v1/remote/lease", { token: laptop.token });
    expect(lapsed.status).toBe(402);
    expect(await errorCode(lapsed)).toBe("subscription_required");
    // Serving and /v1/me are untouched.
    expect((await api(context, "GET", "/v1/me", { token: laptop.token })).status).toBe(200);

    sql("UPDATE plan_overrides SET remote_access = 1");
    sql("UPDATE users SET banned_at = 1");
    const banned = await api(context, "GET", "/v1/remote/lease", { token: laptop.token });
    expect(banned.status).toBe(403);
    expect(await errorCode(banned)).toBe("banned");
  });
});

describe("connecting", () => {
  it("upgrades with the subprotocol and says who is there, both ways", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const { response, socket: sl } = await connect(laptop.token, { "x-localify-proto": "1-2" });
    expect(response.status).toBe(101);
    expect(response.headers.get("sec-websocket-protocol")).toBe(RELAY_PROTOCOL);
    expect(sl!.frames()).toEqual([
      { t: "welcome", v: 1, connId: expect.any(String), machineId: laptop.machineId, peers: [] },
    ]);
    const connectedAt = Date.now();

    vi.setSystemTime(Date.now() + 1000);
    const sr = await online(runner);
    expect(sr.frames()).toEqual([
      {
        t: "welcome",
        v: 1,
        connId: expect.any(String),
        machineId: runner.machineId,
        peers: [
          {
            t: "presence",
            machineId: laptop.machineId,
            state: "online",
            version: "0.30.0",
            proto: { min: 1, max: 2 },
            since: iso(connectedAt),
          },
        ],
      },
    ]);
    expect(sl!.framesOf("presence")).toEqual([
      {
        t: "presence",
        machineId: runner.machineId,
        state: "online",
        version: "0.30.0",
        proto: { min: 1, max: 1 },
        since: iso(Date.now()),
      },
    ]);

    vi.setSystemTime(Date.now() + 1000);
    await context.relays.object(runner.userId).webSocketClose(sr as unknown as WebSocket);
    expect(sl!.framesOf("presence").at(-1)).toMatchObject({
      machineId: runner.machineId,
      state: "offline",
      since: iso(Date.now()),
    });
  });

  it("does no D1 work for a malformed token or a protocol it does not speak", async () => {
    const laptop = await linked();
    const before = context.queries.count;
    for (const authorization of [
      null,
      "Bearer",
      "Bearer lfy_123",
      `Bearer lfy_${"A".repeat(64)}`,
      `Basic ${laptop.token}`,
      `Bearer ${laptop.token}0`,
    ]) {
      const { response } = await connect(null, authorization ? { authorization } : {});
      expect(response.status, String(authorization)).toBe(401);
    }
    for (const headers of <Record<string, string>[]>[
      { "sec-websocket-protocol": "localify-remote.2" },
      { "sec-websocket-protocol": "" },
      { upgrade: "h2c" },
      { "x-localify-proto": "2-1" },
      { "x-localify-proto": "one" },
    ]) {
      const { response } = await connect(laptop.token, headers);
      expect(response.status, JSON.stringify(headers)).toBe(426);
      expect(await errorCode(response)).toBe("unsupported_protocol");
    }
    expect(context.queries.count).toBe(before);
    expect(context.remoteIpLimiter.counts.size).toBe(0);
    expect(context.remoteConnectLimiter.counts.size).toBe(0);
  });

  it("sheds upgrades past either limiter with a spread Retry-After, before D1", async () => {
    const laptop = await linked();
    for (let i = 0; i < 6; i++) await online(laptop);
    const before = context.queries.count;
    const refused = (await connect(laptop.token)).response;
    expect(refused.status).toBe(429);
    expect(await errorCode(refused)).toBe("rate_limited");
    const wait = Number(refused.headers.get("retry-after"));
    expect(wait).toBeGreaterThanOrEqual(30);
    expect(wait).toBeLessThan(60);
    expect(context.queries.count).toBe(before);

    // Per IP, whatever the token.
    context.remoteConnectLimiter.limit = Number.POSITIVE_INFINITY;
    const ip = { "cf-connecting-ip": "203.0.113.7" };
    for (let i = 0; i < 30; i++)
      expect((await connect(laptop.token, ip)).response.status).toBe(101);
    expect((await connect(laptop.token, ip)).response.status).toBe(429);
  });

  it("checks the token, the ban, remote access and the key, in that order", async () => {
    const laptop = await signIn(context);
    const unknown = await connect(`lfy_${"0".repeat(64)}`);
    expect(unknown.response.status).toBe(401);

    const missing = await connect(laptop.token);
    expect(missing.response.status).toBe(409);
    expect(await errorCode(missing.response)).toBe("remote_key_missing");

    sql("UPDATE plan_overrides SET remote_access = 0");
    const lapsed = await connect(laptop.token);
    expect(lapsed.response.status).toBe(402);
    expect(await errorCode(lapsed.response)).toBe("subscription_required");

    sql("UPDATE users SET banned_at = 1");
    const banned = await connect(laptop.token);
    expect(banned.response.status).toBe(403);
    expect(await errorCode(banned.response)).toBe("banned");
  });

  it("sets who the socket is itself, whatever the client sends", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const { socket } = await connect(laptop.token, {
      "x-localify-relay-user": "someone-else",
      "x-localify-relay-machine": runner.machineId,
      "x-localify-relay-checked": String(Date.now() + 10 * AUTHORIZATION_MS),
      "x-localify-version": "<script>",
    });
    expect(socket!.deserializeAttachment()).toEqual({
      userId: laptop.userId,
      machineId: laptop.machineId,
      connId: expect.any(String),
      version: "unknown",
      proto: { min: 1, max: 1 },
      connectedAt: Date.now(),
      authorizedUntil: Date.now() + AUTHORIZATION_MS,
    } satisfies Attachment);
  });

  it("replaces a machine's older socket with 4001, which then says and routes nothing", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const old = await online(laptop);
    const sr = await online(runner);
    const current = await online(laptop);
    expect(old.closed?.code).toBe(CLOSE.replaced);
    expect(current.closed).toBeNull();

    await deliver(laptop, old, hs(runner.machineId));
    expect(sr.framesOf("recv")).toEqual([]);
    await context.relays.object(laptop.userId).webSocketClose(old as unknown as WebSocket);
    expect(sr.framesOf("presence").map((frame) => frame.state)).toEqual(["online"]);
    await deliver(laptop, current, hs(runner.machineId));
    expect(sr.framesOf("recv")).toHaveLength(1);
  });
});

describe("routing", () => {
  it("forwards a frame to the machine it names, from the sender's attachment", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const [sl, sr] = [await online(laptop), await online(runner)];

    await deliver(laptop, sl, hs(runner.machineId, { from: "forged", extra: "dropped" }));
    await deliver(laptop, sl, { t: "send", to: runner.machineId, sid: sid(), kind: "close" });
    await deliver(runner, sr, msg(laptop.machineId));
    expect(sr.framesOf("recv")).toEqual([
      {
        t: "recv",
        from: laptop.machineId,
        sid: sid(),
        kind: "hs",
        purpose: "pair",
        data: "aGVsbG8",
      },
      { t: "recv", from: laptop.machineId, sid: sid(), kind: "close" },
    ]);
    expect(sl.framesOf("recv")).toEqual([
      { t: "recv", from: runner.machineId, sid: sid(), kind: "msg", data: "AAAA" },
    ]);
    expect(sl.framesOf("error")).toEqual([]);
  });

  it("stays within the account: another account's machine is offline from here", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const desk = await linked("someone", 2, "desk");
    const [sl, sd] = [await online(laptop), await online(desk)];
    await deliver(laptop, sl, hs(desk.machineId));
    await deliver(laptop, sl, hs("0".repeat(24)));
    expect(sl.framesOf("error")).toEqual([
      { t: "error", code: "peer_offline", sid: sid(), message: expect.any(String) },
      { t: "error", code: "peer_offline", sid: sid(), message: expect.any(String) },
    ]);
    expect(sd.frames()).toEqual([expect.objectContaining({ t: "welcome", peers: [] })]);
    expect(sl.frames()[0]).toMatchObject({ t: "welcome", peers: [] });
  });

  it("answers a malformed envelope with bad_frame, and closes on what is not a send frame", async () => {
    unlimited();
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const [sl, sr] = [await online(laptop), await online(runner)];
    for (const bad of [
      hs(runner.machineId, { sid: "short" }),
      hs(runner.machineId, { kind: "data" }),
      hs(runner.machineId, { purpose: "other" }),
      hs(runner.machineId, { data: "not base64url!" }),
      { t: "send", to: runner.machineId, sid: sid(), kind: "msg" },
      hs(runner.machineId, { to: "" }),
      hs(laptop.machineId),
    ]) {
      await deliver(laptop, sl, bad);
    }
    const errors = sl.framesOf("error");
    expect(errors.map((error) => error.code)).toEqual(Array(7).fill("bad_frame"));
    expect(errors[0]).not.toHaveProperty("sid");
    expect(errors.slice(1).every((error) => error.sid === sid())).toBe(true);
    expect(sr.framesOf("recv")).toEqual([]);
    expect(sl.closed).toBeNull();

    for (const frame of ["not json", "[]", JSON.stringify({ t: "hello" }), new ArrayBuffer(8)]) {
      const socket = await online(laptop);
      await context.relays
        .object(laptop.userId)
        .webSocketMessage(socket as unknown as WebSocket, frame as string);
      expect(socket.closed?.code, String(frame)).toBe(CLOSE.badFrame);
    }
  });

  it("caps a frame at 64 KiB of UTF-8, closing with 1009 past it", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const [sl, sr] = [await online(laptop), await online(runner)];
    const frame = (length: number) => {
      const base = JSON.stringify({ ...msg(runner.machineId), data: "" }).length;
      return JSON.stringify({ ...msg(runner.machineId), data: "A".repeat(length - base) });
    };
    expect(frame(MAX_FRAME_BYTES)).toHaveLength(MAX_FRAME_BYTES);
    await deliver(laptop, sl, frame(MAX_FRAME_BYTES));
    expect(sr.framesOf("recv")).toHaveLength(1);
    expect(sl.closed).toBeNull();

    await deliver(laptop, sl, frame(MAX_FRAME_BYTES + 1));
    expect(sl.closed?.code).toBe(CLOSE.tooLarge);
    expect(sr.framesOf("presence").at(-1)).toMatchObject({ state: "offline" });
    expect(sr.framesOf("recv")).toHaveLength(1);

    // Fewer UTF-16 units than the cap, more bytes.
    const wide = await online(laptop);
    await deliver(
      laptop,
      wide,
      JSON.stringify({ ...msg(runner.machineId), data: "é".repeat(40_000) }),
    );
    expect(wide.closed?.code).toBe(CLOSE.tooLarge);
  });

  it("refuses frames past the socket's budget, then closes with 4429 and a retry hint", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const [sl, sr] = [await online(laptop), await online(runner)];
    for (let i = 0; i < 200; i++) await deliver(laptop, sl, msg(runner.machineId));
    expect(sr.framesOf("recv")).toHaveLength(200);

    for (let i = 0; i < 49; i++) await deliver(laptop, sl, msg(runner.machineId));
    const refused = sl.framesOf("error");
    expect(refused).toHaveLength(49);
    expect(refused[0]).toEqual({
      t: "error",
      code: "rate_limited",
      sid: sid(),
      message: expect.any(String),
      retryAfter: 1,
    });
    expect(sl.closed).toBeNull();
    await deliver(laptop, sl, msg(runner.machineId));
    expect(sl.closed).toEqual({ code: CLOSE.rateLimited, reason: '{"retryAfter":10}' });
    expect(sr.framesOf("recv")).toHaveLength(200);

    // The budget refills with time: 50 frames a second.
    const again = await online(laptop);
    for (let i = 0; i < 200; i++) await deliver(laptop, again, msg(runner.machineId));
    vi.setSystemTime(Date.now() + 1000);
    for (let i = 0; i < 50; i++) await deliver(laptop, again, msg(runner.machineId));
    expect(again.framesOf("error")).toEqual([]);
  });

  it("leaves pings to the runtime's auto-response, and answers one awake the same way", async () => {
    const laptop = await linked();
    const sl = await online(laptop);
    expect(context.relays.state(laptop.userId).autoResponse).toEqual({
      request: "ping",
      response: "pong",
    });
    const before = context.queries.count;
    vi.setSystemTime(Date.now() + 2 * AUTHORIZATION_MS);
    await deliver(laptop, sl, "ping");
    expect(sl.sent.at(-1)).toBe("pong");
    expect(context.queries.count).toBe(before);
  });
});

describe("lazy authorization", () => {
  it("checks both ends once their minute has passed, in one query, then not for a minute", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const [sl, sr] = [await online(laptop), await online(runner)];
    const checks = recordChecks();
    await deliver(laptop, sl, msg(runner.machineId));
    expect(checks).toEqual([]);

    vi.setSystemTime(Date.now() + AUTHORIZATION_MS);
    await deliver(laptop, sl, msg(runner.machineId));
    expect(checks).toEqual([[laptop.machineId, runner.machineId]]);
    expect((sl.deserializeAttachment() as Attachment).authorizedUntil).toBe(
      Date.now() + AUTHORIZATION_MS,
    );
    await deliver(runner, sr, msg(laptop.machineId));
    expect(checks).toHaveLength(1);
    expect(sr.framesOf("recv")).toHaveLength(2);
    expect(sl.framesOf("recv")).toHaveLength(1);
  });

  it("checks only the end whose minute has passed", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const sl = await online(laptop);
    vi.setSystemTime(Date.now() + AUTHORIZATION_MS / 2);
    await online(runner);
    vi.setSystemTime(Date.now() + AUTHORIZATION_MS / 2);
    const checks = recordChecks();
    await deliver(laptop, sl, msg(runner.machineId));
    expect(checks).toEqual([[laptop.machineId]]);
  });

  it("closes both ends with 4402 once remote access lapsed, forwarding nothing", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const [sl, sr] = [await online(laptop), await online(runner)];
    // An override removed with `cloud:admin`, which writes D1 only.
    sql("DELETE FROM plan_overrides");
    await deliver(laptop, sl, msg(runner.machineId));
    expect(sr.framesOf("recv")).toHaveLength(1);

    vi.setSystemTime(Date.now() + AUTHORIZATION_MS);
    await deliver(laptop, sl, msg(runner.machineId));
    expect(sl.closed?.code).toBe(CLOSE.lapsed);
    expect(sr.closed?.code).toBe(CLOSE.lapsed);
    expect(sr.framesOf("recv")).toHaveLength(1);
    expect((await connect(laptop.token)).response.status).toBe(402);
  });

  it("closes with 4403 once the account is banned", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const [sl, sr] = [await online(laptop), await online(runner)];
    sql("UPDATE users SET banned_at = 1");
    vi.setSystemTime(Date.now() + AUTHORIZATION_MS);
    await deliver(runner, sr, msg(laptop.machineId));
    expect(sl.closed?.code).toBe(CLOSE.banned);
    expect(sr.closed?.code).toBe(CLOSE.banned);
    expect(sl.framesOf("recv")).toEqual([]);
  });

  it("closes a recipient that left the account with 4401, and tells the sender it is offline", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const [sl, sr] = [await online(laptop), await online(runner)];
    // Revoked with no push reaching the relay.
    sql("UPDATE machines SET status = 'revoked' WHERE id = ?", runner.machineId);
    vi.setSystemTime(Date.now() + AUTHORIZATION_MS);
    await deliver(laptop, sl, msg(runner.machineId));
    expect(sr.closed?.code).toBe(CLOSE.revoked);
    expect(sr.framesOf("recv")).toEqual([]);
    expect(sl.closed).toBeNull();
    expect(sl.framesOf("presence").at(-1)).toMatchObject({ state: "offline" });
    expect(sl.framesOf("error")).toEqual([
      { t: "error", code: "peer_offline", sid: sid(), message: expect.any(String) },
    ]);
  });

  it("fails closed when D1 fails: the frame is dropped with unavailable and a retry hint", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const [sl, sr] = [await online(laptop), await online(runner)];
    vi.setSystemTime(Date.now() + AUTHORIZATION_MS);
    context.queries.limit = context.queries.count;
    await deliver(laptop, sl, msg(runner.machineId));
    expect(sl.framesOf("error")).toEqual([
      { t: "error", code: "unavailable", sid: sid(), message: expect.any(String), retryAfter: 5 },
    ]);
    expect(sr.framesOf("recv")).toEqual([]);
    expect([sl.closed, sr.closed]).toEqual([null, null]);

    context.queries.limit = Number.POSITIVE_INFINITY;
    await deliver(laptop, sl, msg(runner.machineId));
    expect(sr.framesOf("recv")).toHaveLength(1);
  });

  it("checks an expired sender before answering anything, a malformed frame included", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    await linked("teo-goulois", 1, "runner");
    const sl = await online(laptop);
    vi.setSystemTime(Date.now() + AUTHORIZATION_MS);
    const checks = recordChecks();
    await deliver(laptop, sl, hs(laptop.machineId));
    expect(checks).toEqual([[laptop.machineId]]);
    expect(sl.framesOf("error")).toEqual([expect.objectContaining({ code: "bad_frame" })]);

    sql("UPDATE users SET banned_at = 1");
    vi.setSystemTime(Date.now() + AUTHORIZATION_MS);
    await deliver(laptop, sl, hs(laptop.machineId, { sid: "short" }));
    expect(checks).toHaveLength(2);
    expect(sl.closed?.code).toBe(CLOSE.banned);
    expect(sl.framesOf("error")).toHaveLength(1);
  });

  it("shares one check of a machine between the frames that need it at once", async () => {
    unlimited();
    const target = await linked("teo-goulois", 1, "target");
    const senders: Machine[] = [];
    for (const name of ["a", "b", "c", "d"]) senders.push(await linked("teo-goulois", 1, name));
    const st = await online(target);
    const sockets: FakeSocket[] = [];
    for (const sender of senders) sockets.push(await online(sender));
    vi.setSystemTime(Date.now() + AUTHORIZATION_MS);
    const checks = recordChecks();
    await Promise.all(
      senders.map((sender, index) => deliver(sender, sockets[index]!, msg(target.machineId))),
    );
    expect(checks).toHaveLength(4);
    expect(checks.flat().filter((id) => id === target.machineId)).toHaveLength(1);
    expect(st.framesOf("recv")).toHaveLength(4);
  });
});

describe("revocation pushes", () => {
  it("close the account's sockets only once the account, read again, has no remote access", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const [sl, sr] = [await online(laptop), await online(runner)];
    const relay = context.relays.object(laptop.userId);

    // A late push for an account entitled again closes nothing.
    await relay.revokeAccount(1);
    expect([sl.closed, sr.closed]).toEqual([null, null]);

    // Nor does one that a newer billing revision overtook.
    sql("DELETE FROM plan_overrides");
    sql(`INSERT INTO billing (user_id, revision, refreshed_at) VALUES (?, 5, 1)`, laptop.userId);
    await relay.revokeAccount(4);
    expect([sl.closed, sr.closed]).toEqual([null, null]);

    vi.setSystemTime(Date.now() + 1);
    await relay.revokeAccount(5);
    expect([sl.closed?.code, sr.closed?.code]).toEqual([CLOSE.lapsed, CLOSE.lapsed]);
    await deliver(laptop, sl, msg(runner.machineId));
    expect(sr.framesOf("recv")).toEqual([]);
    await expect(relay.revokeAccount(Number.NaN)).rejects.toThrow();
  });

  it("close a banned account's sockets with 4403", async () => {
    const laptop = await linked();
    const sl = await online(laptop);
    sql("UPDATE users SET banned_at = 1");
    vi.setSystemTime(Date.now() + 1);
    await context.relays.object(laptop.userId).revokeAccount(0);
    expect(sl.closed?.code).toBe(CLOSE.banned);
  });

  it("spare a socket authorized after the account was read", async () => {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const sl = await online(laptop);
    sql("UPDATE plan_overrides SET remote_access = 0");
    vi.setSystemTime(Date.now() + 1);
    // The read happens now, and its answer comes back late.
    let release!: () => void;
    const gate = new Promise<void>((resolve) => (release = resolve));
    const db = context.env.DB;
    (context.env as { DB: D1Database }).DB = {
      prepare: (text: string) => {
        const statement = db.prepare(text);
        if (text !== REMOTE_REVOKE_SQL) return statement;
        return {
          bind: (...params: unknown[]) => ({
            first: async () => {
              const row = await statement.bind(...params).first();
              await gate;
              return row;
            },
          }),
        };
      },
      batch: (statements: D1PreparedStatement[]) => db.batch(statements),
    } as unknown as D1Database;
    const revoking = context.relays.object(laptop.userId).revokeAccount(0);

    sql("UPDATE plan_overrides SET remote_access = 1");
    const sr = await online(runner);
    release();
    await revoking;
    expect(sl.closed?.code).toBe(CLOSE.lapsed);
    expect(sr.closed).toBeNull();
  });

  it("do nothing, and read nothing, for an account with no socket open", async () => {
    const before = context.queries.count;
    await context.relays.object("nobody").revokeAccount(1);
    await context.relays.object("nobody").revokeMachine("gone");
    expect(context.relays.state("nobody").sockets).toEqual([]);
    expect(context.queries.count).toBe(before);
  });
});

describe("a socket's queue", () => {
  /** Holds the relay's checks in D1 until the returned function is called. */
  function holdChecks(): () => void {
    let release!: () => void;
    const gate = new Promise<void>((resolve) => (release = resolve));
    const db = context.env.DB;
    (context.env as { DB: D1Database }).DB = {
      prepare: (text: string) => {
        const statement = db.prepare(text);
        if (text !== REMOTE_CHECK_SQL) return statement;
        return {
          bind: (...params: unknown[]) => ({
            first: async () => {
              await gate;
              return statement.bind(...params).first();
            },
          }),
        };
      },
      batch: (statements: D1PreparedStatement[]) => db.batch(statements),
    } as unknown as D1Database;
    return release;
  }

  /** Lets every settled delivery run its callbacks. */
  async function flush(): Promise<void> {
    for (let i = 0; i < 5; i++) await new Promise((resolve) => setImmediate(resolve));
  }

  async function expiredPair() {
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const [sl, sr] = [await online(laptop), await online(runner)];
    vi.setSystemTime(Date.now() + AUTHORIZATION_MS);
    return { laptop, runner, sl, sr };
  }

  it("closes on an oversized frame at once, while a check waits for D1", async () => {
    const { laptop, runner, sl, sr } = await expiredPair();
    const release = holdChecks();
    const first = deliver(laptop, sl, msg(runner.machineId));
    const big = JSON.stringify({ ...msg(runner.machineId), data: "A".repeat(MAX_FRAME_BYTES) });
    const oversized = Array.from({ length: 1000 }, () => deliver(laptop, sl, big));
    expect(sl.closed?.code).toBe(CLOSE.tooLarge);
    release();
    await Promise.all([first, ...oversized]);
    expect(sr.framesOf("recv")).toEqual([]);
  });

  it("bounds what waits behind a check, and closes a socket that keeps sending, unanswered", async () => {
    const { laptop, runner, sl, sr } = await expiredPair();
    const release = holdChecks();
    let settled = 0;
    const all = Array.from({ length: 1000 }, () =>
      deliver(laptop, sl, msg(runner.machineId)).then(() => {
        settled += 1;
      }),
    );
    await flush();
    expect(1000 - settled).toBeLessThanOrEqual(MAX_PENDING_FRAMES);
    expect(sl.closed).toEqual({ code: CLOSE.rateLimited, reason: '{"retryAfter":10}' });
    // The socket was due for a check: it got no answer before one.
    expect(sl.framesOf("error")).toEqual([]);
    release();
    await Promise.all(all);
    expect(sr.framesOf("recv")).toEqual([]);
  });

  it("keeps the order of what it queued", async () => {
    const { laptop, runner, sl, sr } = await expiredPair();
    const release = holdChecks();
    const frames = Array.from({ length: 10 }, (_, index) =>
      deliver(laptop, sl, { ...msg(runner.machineId), data: `AAA${index}` }),
    );
    release();
    await Promise.all(frames);
    expect(sr.framesOf("recv").map((frame) => frame.data)).toEqual(
      Array.from({ length: 10 }, (_, index) => `AAA${index}`),
    );
    expect(sl.closed).toBeNull();
  });
});

describe("hibernation", () => {
  it("routes, re-checks and fails closed from the attachments alone, with no storage or timer", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const laptop = await linked("teo-goulois", 1, "laptop");
    const runner = await linked("teo-goulois", 1, "runner");
    const [sl, sr] = [await online(laptop), await online(runner)];
    const connectedAt = Date.now();

    context.relays.evict(laptop.userId);
    await deliver(laptop, sl, hs(runner.machineId));
    expect(sr.framesOf("recv")).toEqual([expect.objectContaining({ from: laptop.machineId })]);

    // A machine that connects after an eviction sees the others as they were.
    const desk = await linked("teo-goulois", 1, "desk");
    context.relays.evict(laptop.userId);
    const sd = await online(desk);
    expect(
      (sd.frames()[0]!.peers as { machineId: string; since: string }[]).map((peer) => [
        peer.machineId,
        peer.since,
      ]),
    ).toEqual([
      [laptop.machineId, iso(connectedAt)],
      [runner.machineId, iso(connectedAt)],
    ]);

    // Authorization expires, the object sleeps, and the next frame checks both ends.
    vi.setSystemTime(Date.now() + AUTHORIZATION_MS);
    context.relays.evict(laptop.userId);
    const checks = recordChecks();
    await deliver(laptop, sl, msg(runner.machineId));
    expect(checks).toEqual([[laptop.machineId, runner.machineId]]);
    expect(sr.framesOf("recv")).toHaveLength(2);

    // The new deadline was in the attachments, so no check for a minute, then D1 fails.
    context.relays.evict(laptop.userId);
    await deliver(runner, sr, msg(laptop.machineId));
    expect(checks).toHaveLength(1);
    vi.setSystemTime(Date.now() + AUTHORIZATION_MS);
    context.relays.evict(laptop.userId);
    context.queries.limit = context.queries.count;
    await deliver(laptop, sl, msg(runner.machineId));
    expect(sl.framesOf("error").at(-1)).toMatchObject({ code: "unavailable", retryAfter: 5 });
    expect(sr.framesOf("recv")).toHaveLength(2);

    // A socket that closed meanwhile is not rebuilt.
    context.queries.limit = Number.POSITIVE_INFINITY;
    sd.readyState = 3;
    context.relays.evict(laptop.userId);
    await deliver(laptop, sl, msg(desk.machineId));
    expect(sl.framesOf("error").at(-1)).toMatchObject({ code: "peer_offline" });

    expect(context.relays.state(laptop.userId).storageUsed).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("a reconnect storm", () => {
  it("stays bounded: malformed tokens never reach D1, two queries an upgrade, none a first frame", async () => {
    // 200 daemons across 20 accounts, each account behind one address.
    const daemons: (Machine & { ip: string })[] = [];
    for (let account = 0; account < 20; account++) {
      const userId = `user-${account}`;
      sql(
        `INSERT INTO users (id, github_id, login, created_at) VALUES (?, ?, ?, 1)`,
        userId,
        1000 + account,
        `login-${account}`,
      );
      sql(
        `INSERT INTO plan_overrides (user_id, serving_machines, public_urls, remote_access, reason, created_at)
         VALUES (?, 0, 0, 1, 'test', 1)`,
        userId,
      );
      for (let machine = 0; machine < 10; machine++) {
        const machineId = randomBytes(12).toString("hex");
        const token = `lfy_${randomBytes(32).toString("hex")}`;
        const key = newKey();
        sql(
          `INSERT INTO machines (id, user_id, name, status, ingress_port, created_at, remote_key)
           VALUES (?, ?, ?, 'active', 7381, ?, ?)`,
          machineId,
          userId,
          `machine-${machine}`,
          machine,
          key,
        );
        sql(
          `INSERT INTO tokens (hash, user_id, machine_id, created_at) VALUES (?, ?, ?, 1)`,
          await sha256Hex(token),
          userId,
          machineId,
        );
        daemons.push({ userId, machineId, token, key, ip: `198.51.100.${account}` });
      }
    }
    const connectAll = () =>
      Promise.all(
        daemons.map((daemon) => connect(daemon.token, { "cf-connecting-ip": daemon.ip })),
      );
    for (const { response } of await connectAll()) expect(response.status).toBe(101);

    // A deploy drops every socket with 1012, and the objects start over.
    for (const state of context.relays.states.values()) {
      for (const socket of state.sockets) socket.close(1012, "deploy");
    }
    context.relays.states.clear();
    context.relays.objects.clear();

    const before = context.queries.count;
    const [reconnected, malformed] = await Promise.all([
      connectAll(),
      Promise.all(
        Array.from({ length: 100 }, (_, index) =>
          connect(null, { authorization: `Bearer lfy_${index}`, "cf-connecting-ip": "192.0.2.1" }),
        ),
      ),
    ]);
    expect(malformed.every(({ response }) => response.status === 401)).toBe(true);
    expect(reconnected.every(({ response }) => response.status === 101)).toBe(true);
    expect(context.queries.count - before).toBe(2 * daemons.length);

    // Every relay holds its 10 machines again; their first frames cost no query.
    for (const state of context.relays.states.values()) expect(state.sockets).toHaveLength(10);
    const sockets = reconnected.map(({ socket }) => socket!);
    const upgraded = context.queries.count;
    await Promise.all(
      daemons.map((daemon, index) => {
        const peer = daemons[index % 10 === 9 ? index - 9 : index + 1]!;
        return deliver(daemon, sockets[index]!, msg(peer.machineId));
      }),
    );
    expect(context.queries.count).toBe(upgraded);
    // Nothing from before the deploy comes back: one frame each, this round's.
    expect(sockets.every((socket) => socket.framesOf("recv").length === 1)).toBe(true);

    // A daemon that retries too fast is shed with a hint, before D1.
    for (let i = 0; i < 4; i++) {
      expect(
        (await connect(daemons[0]!.token, { "cf-connecting-ip": daemons[0]!.ip })).response.status,
      ).toBe(101);
    }
    const shed = context.queries.count;
    const refused = (await connect(daemons[0]!.token, { "cf-connecting-ip": daemons[0]!.ip }))
      .response;
    expect(refused.status).toBe(429);
    expect(Number(refused.headers.get("retry-after"))).toBeGreaterThanOrEqual(30);
    expect(context.queries.count).toBe(shed);
  });
});

describe("migration 0006", () => {
  it("gives every existing override remote access, and lets no two machines share a key", () => {
    const sqlite = new DatabaseSync(":memory:", { enableForeignKeyConstraints: true });
    applyMigrations(sqlite, (file) => file < "0006");
    sqlite.exec(`
      INSERT INTO users (id, github_id, login, created_at) VALUES ('u', 1, 'teo', 1), ('v', 2, 'ana', 1);
      INSERT INTO plan_overrides (user_id, serving_machines, public_urls, reason, created_at)
      VALUES ('u', 5, 10, 'team', 1);`);
    applyMigrations(sqlite, (file) => file.startsWith("0006"));
    sqlite.exec(`INSERT INTO plan_overrides (user_id, serving_machines, public_urls, reason, created_at)
                 VALUES ('v', 1, 1, 'later', 1)`);
    expect(
      sqlite.prepare("SELECT user_id, remote_access FROM plan_overrides ORDER BY user_id").all(),
    ).toEqual([
      { user_id: "u", remote_access: 1 },
      { user_id: "v", remote_access: 0 },
    ]);

    const machine = sqlite.prepare(
      `INSERT INTO machines (id, user_id, name, status, ingress_port, created_at, remote_key)
       VALUES (?, 'u', ?, 'active', 7381, 1, ?)`,
    );
    machine.run("m1", "a", null);
    machine.run("m2", "b", null);
    machine.run("m3", "c", "key");
    expect(() => machine.run("m4", "d", "key")).toThrow(/UNIQUE/);
  });
});
