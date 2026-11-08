/// <reference types="node" />
// Billing through Polar (docs/public-urls.md, "Billing"; ADR 0024): the
// webhooks, the refresh, the lifecycle, checkouts, the portal, duplicate
// subscriptions and the reconciliation, against a fake Polar.

import { createHmac, randomBytes } from "node:crypto";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createCheckout, reconcileBilling, refreshBilling, runBillingJobs } from "../src/billing";
import { PolarApi } from "../src/polar";
import {
  api,
  createContext,
  jsonResponse,
  POLAR_WEBHOOK_SECRET,
  serve,
  settle,
  signIn,
  type FakeSocket,
  type TestContext,
} from "./support";

const POLAR = "https://polar.test";
const MONTHLY = "prod-monthly";
const YEARLY = "prod-yearly";
const DAY = 24 * 60 * 60 * 1000;
const GRACE = 7 * DAY;

let context: TestContext;
let polar: FakePolar;

beforeEach(() => {
  context = createContext();
  polar = fakePolar(context);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

// --- A fake Polar ---

type Raw = Record<string, unknown>;

type FakePolar = {
  /** Every subscription of the organization. */
  subscriptions: Raw[];
  /** Answers `POST /v1/checkouts/`. */
  checkout: (body: Raw) => Response | Promise<Response>;
  revoked: string[];
  /** Subscriptions set to cancel at period end, with the body that asked. */
  canceled: { id: string; body: Raw }[];
};

function page(items: Raw[], url: string): Response {
  const query = new URL(url).searchParams;
  const number = Number(query.get("page") ?? "1");
  const limit = Number(query.get("limit") ?? "10");
  return jsonResponse({
    items: items.slice((number - 1) * limit, number * limit),
    pagination: {
      total_count: items.length,
      max_page: Math.max(1, Math.ceil(items.length / limit)),
    },
  });
}

function fakePolar(context: TestContext): FakePolar {
  let checkouts = 0;
  const fake: FakePolar = {
    subscriptions: [],
    checkout: () =>
      jsonResponse(
        {
          id: `checkout-${++checkouts}`,
          url: `${POLAR}/checkout/${checkouts}`,
          expires_at: iso(Date.now() + 60 * 60 * 1000),
        },
        201,
      ),
    revoked: [],
    canceled: [],
  };
  context.routes.set(`GET ${POLAR}/v1/subscriptions/`, (_body, url) => {
    const external = new URL(url).searchParams.get("external_customer_id");
    const items =
      external === null
        ? fake.subscriptions
        : fake.subscriptions.filter(
            (s) => (s.customer as { external_id: string }).external_id === external,
          );
    return page(items, url);
  });
  context.routes.set(`DELETE ${POLAR}/v1/subscriptions/`, (_body, url) => {
    const id = url.slice(`${POLAR}/v1/subscriptions/`.length);
    const subscription = fake.subscriptions.find((s) => s.id === id);
    if (!subscription) return jsonResponse({ error: "ResourceNotFound", detail: "Not found" }, 404);
    if (subscription.ended_at) {
      return jsonResponse({ error: "AlreadyCanceledSubscription", detail: "Already" }, 403);
    }
    fake.revoked.push(id);
    Object.assign(subscription, { status: "canceled", ended_at: iso(Date.now()) });
    return jsonResponse(subscription);
  });
  context.routes.set(`PATCH ${POLAR}/v1/subscriptions/`, (body, url) => {
    const id = url.slice(`${POLAR}/v1/subscriptions/`.length);
    const subscription = fake.subscriptions.find((s) => s.id === id);
    if (!subscription) return jsonResponse({ error: "ResourceNotFound", detail: "Not found" }, 404);
    if (subscription.ended_at) {
      return jsonResponse({ error: "InactiveSubscription", detail: "Inactive" }, 403);
    }
    const update = JSON.parse(body ?? "{}") as { cancel_at_period_end: boolean };
    if (update.cancel_at_period_end && subscription.cancel_at_period_end) {
      return jsonResponse({ error: "AlreadyCanceledSubscription", detail: "Already" }, 403);
    }
    fake.canceled.push({ id, body: update });
    subscription.cancel_at_period_end = update.cancel_at_period_end;
    return jsonResponse(subscription);
  });
  context.routes.set(`POST ${POLAR}/v1/checkouts/`, (body) =>
    fake.checkout(JSON.parse(body ?? "{}") as Raw),
  );
  context.routes.set(`POST ${POLAR}/v1/customer-sessions/`, (body) => {
    const { external_customer_id } = JSON.parse(body ?? "{}") as { external_customer_id: string };
    return jsonResponse({ customer_portal_url: `${POLAR}/portal/${external_customer_id}` }, 201);
  });
  return fake;
}

function iso(ms: number | null): string | null {
  return ms === null ? null : new Date(ms).toISOString();
}

let subscriptionCount = 0;

/** A Polar subscription of `userId`: monthly Pro, active, unless `fields` say otherwise. */
function subscription(userId: string, fields: Raw = {}): Raw {
  const now = Date.now();
  const subscription: Raw = {
    id: `sub-${++subscriptionCount}`,
    status: "active",
    product_id: MONTHLY,
    customer_id: `cus-${userId}`,
    recurring_interval: "month",
    cancel_at_period_end: false,
    current_period_end: iso(now + 30 * DAY),
    trial_start: null,
    trial_end: null,
    started_at: iso(now - DAY),
    ends_at: null,
    ended_at: null,
    past_due_at: null,
    units: null,
    customer: { id: `cus-${userId}`, external_id: userId },
    ...fields,
  };
  polar.subscriptions.push(subscription);
  return subscription;
}

function polarCalls(method: string, path: string) {
  return context.calls.filter(
    (call) => call.method === method && call.url.startsWith(`${POLAR}${path}`),
  );
}

// --- Helpers ---

function get<T>(sql: string, ...params: (string | number | null)[]): T {
  return context.sqlite.prepare(sql).get(...params) as T;
}

function run(sql: string, ...params: (string | number | null)[]): void {
  context.sqlite.prepare(sql).run(...params);
}

function addUser(id: string, githubId: number): void {
  run(
    "INSERT INTO users (id, github_id, login, created_at) VALUES (?, ?, ?, 1)",
    id,
    githubId,
    `user-${githubId}`,
  );
}

async function refresh(userId: string) {
  return refreshBilling(context.env, userId, new PolarApi(context.env));
}

async function plan(token: string): Promise<Record<string, unknown>> {
  const response = await api(context, "GET", "/v1/me", { token });
  return ((await response.json()) as { plan: Record<string, unknown> }).plan;
}

function sign(id: string, timestamp: string, body: string, secret = POLAR_WEBHOOK_SECRET): string {
  const key = Buffer.from(secret.slice("whsec_".length), "base64");
  return createHmac("sha256", key).update(`${id}.${timestamp}.${body}`).digest("base64");
}

let deliveryCount = 0;

/** Delivers a webhook signed for `body`; `sent` replaces what travels. */
async function deliver(
  event: Raw,
  options: {
    id?: string;
    timestamp?: number;
    signature?: string;
    sent?: string;
  } = {},
): Promise<Response> {
  const { default: worker } = await import("../src/index");
  const id = options.id ?? `msg_${++deliveryCount}`;
  const timestamp = String(Math.floor((options.timestamp ?? Date.now()) / 1000));
  const body = JSON.stringify(event);
  const request = new Request("https://api.localify.dev/v1/webhooks/polar", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "webhook-id": id,
      "webhook-timestamp": timestamp,
      "webhook-signature": options.signature ?? `v1,${sign(id, timestamp, body)}`,
    },
    body: options.sent ?? body,
  });
  return worker.fetch(request as never, context.env, context.ctx);
}

function subscriptionEvent(userId: string | null, type = "subscription.updated"): Raw {
  return {
    type,
    timestamp: iso(Date.now()),
    data: { id: "sub-x", status: "active", customer: { id: "cus", external_id: userId } },
  };
}

function events(): {
  id: string;
  user_id: string | null;
  processed_at: number | null;
  attempts: number;
}[] {
  return context.sqlite
    .prepare("SELECT id, user_id, processed_at, attempts FROM webhook_events ORDER BY seq")
    .all() as never;
}

function billingRow(userId: string): Record<string, unknown> | undefined {
  return context.sqlite.prepare("SELECT * FROM billing WHERE user_id = ?").get(userId) as never;
}

// --- Webhooks ---

describe("webhooks", () => {
  it("accept a valid signature, record the event and refresh its user", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    subscription(userId);

    const response = await deliver(subscriptionEvent(userId), { id: "msg_valid" });
    expect(response.status).toBe(202);
    await settle(context);

    expect(events()).toMatchObject([{ id: "msg_valid", user_id: userId }]);
    expect(events()[0]!.processed_at).not.toBeNull();
    expect(billingRow(userId)).toMatchObject({ pro_status: "active", revision: 1 });
    // Every call pins the API version.
    const [call] = polarCalls("GET", "/v1/subscriptions/");
    expect(call!.headers.get("polar-version")).toBe("2026-10");
    expect(call!.headers.get("authorization")).toBe("Bearer polar-token");
    expect(new URL(call!.url).searchParams.get("external_customer_id")).toBe(userId);
  });

  it("refuse a bad signature, a tampered body, a stale timestamp or another secret", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const event = subscriptionEvent(userId);

    expect((await deliver(event, { signature: "v1,bm90LWEtc2lnbmF0dXJl" })).status).toBe(403);
    // Signed for one body, sent with another: one character changed.
    const other = userId.endsWith("0") ? "1" : "0";
    const tampered = JSON.stringify(event).replace(userId, `${userId.slice(0, -1)}${other}`);
    expect(tampered).not.toBe(JSON.stringify(event));
    expect((await deliver(event, { sent: tampered })).status).toBe(403);
    // A space more is a different raw body too.
    expect((await deliver(event, { sent: `${JSON.stringify(event)} ` })).status).toBe(403);
    expect((await deliver(event, { timestamp: Date.now() - 6 * 60 * 1000 })).status).toBe(403);
    const otherSecret = `whsec_${Buffer.from("another-secret").toString("base64")}`;
    const id = "msg_other";
    const now = Date.now();
    const timestamp = String(Math.floor(now / 1000));
    expect(
      (
        await deliver(event, {
          id,
          timestamp: now,
          signature: `v1,${sign(id, timestamp, JSON.stringify(event), otherSecret)}`,
        })
      ).status,
    ).toBe(403);

    expect(events()).toEqual([]);
    expect(polarCalls("GET", "/v1/subscriptions/")).toEqual([]);
  });

  it("accept any of several signatures, as during a secret rotation", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const event = subscriptionEvent(userId);
    const now = Date.now();
    const timestamp = String(Math.floor(now / 1000));
    const valid = sign("msg_rotation", timestamp, JSON.stringify(event));
    const response = await deliver(event, {
      id: "msg_rotation",
      timestamp: now,
      signature: `v1,bm90LWl0 v1,${valid}`,
    });
    expect(response.status).toBe(202);
  });

  it("answer 503 while POLAR_WEBHOOK_SECRET is not set", async () => {
    context.env = { ...context.env, POLAR_WEBHOOK_SECRET: "" } as Env;
    vi.spyOn(console, "error").mockImplementation(() => {});
    expect((await deliver(subscriptionEvent("u"))).status).toBe(503);
  });

  it("recognize a repeated delivery and refresh once", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    subscription(userId);
    expect((await deliver(subscriptionEvent(userId), { id: "msg_same" })).status).toBe(202);
    await settle(context);
    expect((await deliver(subscriptionEvent(userId), { id: "msg_same" })).status).toBe(202);
    await settle(context);

    expect(events()).toHaveLength(1);
    expect(polarCalls("GET", "/v1/subscriptions/")).toHaveLength(1);
  });

  it("take state from Polar, so deliveries out of order change nothing", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    // Polar now says canceled; the deliveries arrive newest first.
    subscription(userId, {
      status: "canceled",
      ended_at: iso(Date.now() - 1000),
      ends_at: iso(Date.now() - 1000),
    });
    await deliver(subscriptionEvent(userId, "subscription.revoked"));
    await deliver(subscriptionEvent(userId, "subscription.created"));
    await deliver(subscriptionEvent(userId, "subscription.active"));
    await settle(context);

    expect(billingRow(userId)).toMatchObject({ pro_status: "canceled" });
    expect(events().every((event) => event.processed_at !== null)).toBe(true);
  });

  it("record events without a known user as processed, and ignore other types", async () => {
    await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    expect((await deliver(subscriptionEvent("someone-else"))).status).toBe(202);
    expect((await deliver(subscriptionEvent(null))).status).toBe(202);
    expect((await deliver({ type: "order.paid", data: { id: "o" } })).status).toBe(202);
    await settle(context);

    expect(events()).toHaveLength(3);
    expect(events().every((event) => event.user_id === null && event.processed_at !== null)).toBe(
      true,
    );
    expect(polarCalls("GET", "/v1/subscriptions/")).toEqual([]);
  });

  it("name the user of customer.state_changed by data.external_id", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    subscription(userId);
    await deliver({ type: "customer.state_changed", data: { id: "cus", external_id: userId } });
    await settle(context);
    expect(billingRow(userId)).toMatchObject({ pro_status: "active" });
  });

  it("leave the events of a failed refresh pending, and retry them with backoff", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    subscription(userId);
    context.routes.set(`GET ${POLAR}/v1/subscriptions/`, () =>
      jsonResponse({ error: "InternalServerError" }, 500),
    );
    vi.spyOn(console, "error").mockImplementation(() => {});
    await deliver(subscriptionEvent(userId), { id: "msg_fails" });
    await settle(context);

    const pending = get<{ processed_at: number | null; attempts: number; next_attempt_at: number }>(
      "SELECT processed_at, attempts, next_attempt_at FROM webhook_events WHERE id = 'msg_fails'",
    );
    expect(pending.processed_at).toBeNull();
    expect(pending.attempts).toBe(1);
    expect(pending.next_attempt_at).toBeGreaterThan(Date.now());
    expect(billingRow(userId)).toBeUndefined();

    // Not due yet: the job leaves it.
    polar = fakePolar(context);
    subscription(userId);
    await runBillingJobs(context.env, Date.now());
    expect(billingRow(userId)).toBeUndefined();

    // Due: the job refreshes the user and the event is processed.
    await runBillingJobs(context.env, pending.next_attempt_at);
    expect(billingRow(userId)).toMatchObject({ pro_status: "active" });
    expect(events()[0]!.processed_at).not.toBeNull();
  });
});

// --- The refresh ---

describe("the refresh", () => {
  /** Holds each subscription listing until the test releases it, with the state at call time. */
  function holdListings(): (() => void)[] {
    const held: (() => void)[] = [];
    context.routes.set(`GET ${POLAR}/v1/subscriptions/`, (_body, url) => {
      const answer = page(
        polar.subscriptions.map((s) => ({ ...s })),
        url,
      );
      return new Promise<Response>((resolve) => held.push(() => resolve(answer)));
    });
    return held;
  }

  it("never lets an older refresh win, whichever finishes last", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const held = holdListings();
    const sub = subscription(userId);

    // The older refresh reads `active`; the newer one, started after, `canceled`.
    const older = refresh(userId);
    await vi.waitFor(() => expect(held).toHaveLength(1));
    Object.assign(sub, { status: "canceled", ended_at: iso(Date.now()) });
    const newer = refresh(userId);
    await vi.waitFor(() => expect(held).toHaveLength(2));

    held[1]!();
    expect(await newer).toBe("committed");
    held[0]!();
    expect(await older).toBe("superseded");
    expect(billingRow(userId)).toMatchObject({ pro_status: "canceled", revision: 2 });
  });

  it("closes the account's relay sockets with 4402 once a refresh ends remote access", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    const sub = subscription(userId);
    expect(await refresh(userId)).toBe("committed");
    const publicKey = randomBytes(32).toString("base64url");
    expect(
      (await api(context, "PUT", "/v1/machines/self/remote-key", { token, body: { publicKey } }))
        .status,
    ).toBe(204);
    const connected = await api(context, "GET", "/v1/remote/connect", {
      token,
      headers: { upgrade: "websocket", "sec-websocket-protocol": "localify-remote.1" },
    });
    expect(connected.status).toBe(101);
    const socket = (connected as unknown as { webSocket: FakeSocket }).webSocket.peer!;

    // A change that keeps remote access closes nothing.
    Object.assign(sub, { current_period_end: iso(Date.now() + 40 * DAY) });
    expect(await refresh(userId)).toBe("committed");
    expect(socket.closed).toBeNull();

    Object.assign(sub, { status: "canceled", ended_at: iso(Date.now()) });
    expect(await refresh(userId)).toBe("committed");
    expect(socket.closed?.code).toBe(4402);
  });

  it("lets a late revocation of a lapse close nothing once a newer refresh gave access back", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    const sub = subscription(userId);
    expect(await refresh(userId)).toBe("committed");
    const publicKey = randomBytes(32).toString("base64url");
    await api(context, "PUT", "/v1/machines/self/remote-key", { token, body: { publicKey } });

    // Refresh A sees the lapse; its push to the relay is held.
    let release!: () => void;
    const gate = new Promise<void>((resolve) => (release = resolve));
    let held = false;
    const relays = context.env.RELAY;
    (context.env as { RELAY: unknown }).RELAY = {
      idFromName: (name: string) => relays.idFromName(name),
      get: (id: DurableObjectId) => {
        const relay = relays.get(id);
        return {
          fetch: (request: Request) => relay.fetch(request),
          revokeMachine: (machineId: string) => relay.revokeMachine(machineId),
          revokeAccount: async (revision: number) => {
            held = true;
            await gate;
            return relay.revokeAccount(revision);
          },
        };
      },
    };
    Object.assign(sub, { status: "canceled", ended_at: iso(Date.now()) });
    const a = refresh(userId);
    await vi.waitFor(() => expect(held).toBe(true));

    // Refresh B: a new subscription gives access back, and the machine connects.
    subscription(userId);
    expect(await refresh(userId)).toBe("committed");
    const connected = await api(context, "GET", "/v1/remote/connect", {
      token,
      headers: { upgrade: "websocket", "sec-websocket-protocol": "localify-remote.1" },
    });
    expect(connected.status).toBe(101);
    const socket = (connected as unknown as { webSocket: FakeSocket }).webSocket.peer!;

    release();
    expect(await a).toBe("committed");
    expect(socket.closed).toBeNull();
  });

  it("lets a newer refresh overwrite an older one that finished first", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const held = holdListings();
    const sub = subscription(userId);
    const older = refresh(userId);
    await vi.waitFor(() => expect(held).toHaveLength(1));
    Object.assign(sub, { status: "canceled", ended_at: iso(Date.now()) });
    const newer = refresh(userId);
    await vi.waitFor(() => expect(held).toHaveLength(2));

    held[0]!();
    expect(await older).toBe("committed");
    held[1]!();
    expect(await newer).toBe("committed");
    expect(billingRow(userId)).toMatchObject({ pro_status: "canceled", revision: 2 });
  });

  function duplicateIds(): string[] {
    return (
      context.sqlite
        .prepare("SELECT subscription_id FROM duplicate_subscriptions ORDER BY subscription_id")
        .all() as { subscription_id: string }[]
    ).map((row) => row.subscription_id);
  }

  it("never records both subscriptions when two refreshes cross", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const held = holdListings();
    const x = subscription(userId, {
      product_id: YEARLY,
      recurring_interval: "year",
      current_period_end: iso(Date.now() + 360 * DAY),
    });
    const y = subscription(userId, { current_period_end: iso(Date.now() + 20 * DAY) });
    vi.spyOn(console, "error").mockImplementation(() => {});

    // A reads both renewing: keeps X, whose period ends later, and records Y.
    const a = refresh(userId);
    await vi.waitFor(() => expect(held).toHaveLength(1));
    // X is canceled at its period end: B keeps Y, which renews, and would record X.
    Object.assign(x, { cancel_at_period_end: true, ends_at: x.current_period_end });
    const b = refresh(userId);
    await vi.waitFor(() => expect(held).toHaveLength(2));

    held[0]!();
    expect(await a).toBe("committed");
    held[1]!();
    // B meets the record of the one it keeps, and starts again from Polar.
    await vi.waitFor(() => expect(held).toHaveLength(3));
    held[2]!();
    expect(await b).toBe("committed");

    expect(duplicateIds()).toEqual([y.id]);
    expect(billingRow(userId)).toMatchObject({ pro_subscription_id: x.id, revision: 3 });
  });

  it("never records as a duplicate the subscription billing keeps", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const held = holdListings();
    const x = subscription(userId, {
      product_id: YEARLY,
      recurring_interval: "year",
      current_period_end: iso(Date.now() + 360 * DAY),
    });
    vi.spyOn(console, "error").mockImplementation(() => {});

    // A sees X alone, and keeps it.
    const a = refresh(userId);
    await vi.waitFor(() => expect(held).toHaveLength(1));
    // X is canceled at its period end, and Y appears: B would keep Y and record X.
    Object.assign(x, { cancel_at_period_end: true, ends_at: x.current_period_end });
    const y = subscription(userId, { current_period_end: iso(Date.now() + 20 * DAY) });
    const b = refresh(userId);
    await vi.waitFor(() => expect(held).toHaveLength(2));

    held[0]!();
    expect(await a).toBe("committed");
    held[1]!();
    await vi.waitFor(() => expect(held).toHaveLength(3));
    held[2]!();
    expect(await b).toBe("committed");

    // The one billing keeps stays kept while it is entitled.
    expect(duplicateIds()).toEqual([y.id]);
    expect(billingRow(userId)).toMatchObject({ pro_subscription_id: x.id });
  });

  /** A user whose Pro plan serves one URL from a machine with a ready tunnel. */
  async function servingUser() {
    const { token, userId, machineId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    const sub = subscription(userId);
    await refresh(userId);
    const created = await api(context, "POST", "/v1/public-urls", {
      token,
      body: {
        localDomain: "journal.localify",
        slug: "journal-race",
        protection: { mode: "public" },
      },
    });
    expect(created.status).toBe(201);
    await settle(context);
    await serve(context, token);
    return { userId, machineId, sub };
  }

  function servingState(userId: string, machineId: string) {
    return {
      suspended: get<{ suspended_at: number | null }>(
        "SELECT suspended_at FROM public_urls WHERE user_id = ?",
        userId,
      ).suspended_at,
      rotation: get<{ rotate_requested_at: number | null }>(
        "SELECT rotate_requested_at FROM machines WHERE id = ?",
        machineId,
      ).rotate_requested_at,
      owed: get<{ serving_due_at: number | null }>(
        "SELECT serving_due_at FROM billing_revisions WHERE user_id = ?",
        userId,
      ).serving_due_at,
    };
  }

  /** The test D1, where each batch `matches` accepts is held until released, or fails. */
  function gatedDb(matches: (sql: string[]) => boolean, mode: "hold" | "fail") {
    const held: (() => void)[] = [];
    const db = context.env.DB;
    const gated = {
      prepare: (sql: string) => db.prepare(sql),
      batch: async (statements: D1PreparedStatement[]) => {
        if (matches(statements.map((s) => (s as unknown as { sql: string }).sql))) {
          if (mode === "fail") throw new Error("D1 refused the batch");
          await new Promise<void>((resolve) => held.push(resolve));
        }
        return db.batch(statements);
      },
    } as unknown as D1Database;
    return { env: { ...context.env, DB: gated } as Env, held };
  }

  /** The batch of a billing refresh's serving reconcile. */
  const reconcileBatch = (sql: string[]) => sql.some((s) => s.includes("serving_due_at = NULL"));

  it("lets no overtaken refresh suspend a URL that a newer one keeps serving", async () => {
    const { userId, machineId, sub } = await servingUser();

    // The older refresh reads a revocation, commits it, and stops after its reconcile read.
    Object.assign(sub, { status: "canceled", ended_at: iso(Date.now()) });
    const gate = gatedDb(reconcileBatch, "hold");
    const older = refreshBilling(gate.env, userId, new PolarApi(gate.env));
    await vi.waitFor(() => expect(gate.held).toHaveLength(1));

    // Polar restores the plan, and a newer refresh commits it.
    Object.assign(sub, { status: "active", ended_at: null });
    expect(await refresh(userId)).toBe("committed");

    gate.held[0]!();
    expect(await older).toBe("committed");
    expect(servingState(userId, machineId)).toEqual({
      suspended: null,
      rotation: null,
      owed: null,
    });
  });

  it("keeps the serving reconcile owed, and the events pending, until it commits", async () => {
    const { userId, machineId, sub } = await servingUser();
    run(
      `INSERT INTO webhook_events (id, type, user_id, received_at, next_attempt_at)
       VALUES ('msg_owed', 'subscription.revoked', ?, 1, 1)`,
      userId,
    );
    Object.assign(sub, { status: "canceled", ended_at: iso(Date.now()) });
    const gate = gatedDb(reconcileBatch, "fail");
    await expect(refreshBilling(gate.env, userId, new PolarApi(gate.env))).rejects.toThrow(
      /refused/,
    );

    // The snapshot is in, the reconcile owed, and the event pending.
    expect(billingRow(userId)).toMatchObject({ pro_status: "canceled" });
    expect(servingState(userId, machineId)).toMatchObject({ suspended: null });
    expect(servingState(userId, machineId).owed).not.toBeNull();
    expect(events().find((event) => event.id === "msg_owed")!.processed_at).toBeNull();

    // The 5-minute job pays the debt.
    await runBillingJobs(context.env, Date.now());
    const state = servingState(userId, machineId);
    expect(state.suspended).not.toBeNull();
    expect(state.rotation).not.toBeNull();
    expect(state.owed).toBeNull();
  });

  it("waits out Polar's rate limit without counting it as a failure", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    run(
      `INSERT INTO webhook_events (id, type, user_id, received_at, next_attempt_at)
       VALUES ('msg_limited', 'subscription.updated', ?, 1, 1)`,
      userId,
    );
    context.routes.set(
      `GET ${POLAR}/v1/subscriptions/`,
      () =>
        new Response(JSON.stringify({ error: "TooManyRequests" }), {
          status: 429,
          headers: { "retry-after": "120" },
        }),
    );
    // Queued by the daily job as well.
    run("INSERT INTO billing_revisions (user_id, last, check_due_at) VALUES (?, 0, 5)", userId);
    const now = Date.now();
    await expect(
      refreshBilling(context.env, userId, new PolarApi(context.env), now),
    ).rejects.toThrow();
    expect(
      get("SELECT attempts, next_attempt_at FROM webhook_events WHERE id = 'msg_limited'"),
    ).toEqual({ attempts: 0, next_attempt_at: now + 120_000 });
    // Its turn in the daily queue waits the same, instead of coming back at once.
    expect(get("SELECT check_due_at FROM billing_revisions WHERE user_id = ?", userId)).toEqual({
      check_due_at: now + 120_000,
    });
    vi.spyOn(console, "warn").mockImplementation(() => {});
    await runBillingJobs(context.env, now + 60_000);
    expect(polarCalls("GET", "/v1/subscriptions/")).toHaveLength(1);
  });

  it("sends an owed reconcile that fails to the back, so the others get their turn", async () => {
    for (let i = 0; i < 11; i++) {
      const id = `owed-${String(i).padStart(2, "0")}`;
      addUser(id, 2000 + i);
      run("INSERT INTO billing (user_id, revision, refreshed_at) VALUES (?, 1, 1)", id);
      run(
        "INSERT INTO billing_revisions (user_id, last, serving_due_at) VALUES (?, 1, ?)",
        id,
        1 + i,
      );
    }
    // The reconciles of the 10 owed longest keep failing.
    const failing = new Set(
      Array.from({ length: 10 }, (_, i) => `owed-${String(i).padStart(2, "0")}`),
    );
    const db = context.env.DB;
    const env = {
      ...context.env,
      DB: {
        prepare: (sql: string) => db.prepare(sql),
        batch: async (statements: D1PreparedStatement[]) => {
          const params = statements.flatMap((s) => (s as unknown as { params: unknown[] }).params);
          const sql = statements.map((s) => (s as unknown as { sql: string }).sql);
          if (reconcileBatch(sql) && params.some((p) => failing.has(String(p)))) {
            throw new Error("D1 refused the batch");
          }
          return db.batch(statements);
        },
      } as unknown as D1Database,
    } as Env;
    vi.spyOn(console, "error").mockImplementation(() => {});
    const owed = (id: string) =>
      get<{ serving_due_at: number | null }>(
        "SELECT serving_due_at FROM billing_revisions WHERE user_id = ?",
        id,
      ).serving_due_at;

    const now = Date.now();
    await runBillingJobs(env, now);
    expect(owed("owed-10")).toBe(11);
    expect(owed("owed-00")).toBe(now);
    // The eleventh gets its turn on the next run, and the failing ones are still retried.
    await runBillingJobs(env, now + 5 * 60 * 1000);
    expect(owed("owed-10")).toBeNull();
    expect(owed("owed-00")).toBe(now + 5 * 60 * 1000);
  });

  it("leaves its events pending while a serving reconcile is still owed", async () => {
    const { userId, machineId, sub } = await servingUser();
    run(
      `INSERT INTO webhook_events (id, type, user_id, received_at, next_attempt_at)
       VALUES ('msg_older', 'subscription.revoked', ?, 1, 1)`,
      userId,
    );
    // The older refresh commits a revocation and stops after its reconcile read.
    Object.assign(sub, { status: "canceled", ended_at: iso(Date.now()) });
    const older = gatedDb(reconcileBatch, "hold");
    const olderRefresh = refreshBilling(older.env, userId, new PolarApi(older.env));
    await vi.waitFor(() => expect(older.held).toHaveLength(1));

    // A newer one commits too, and its reconcile fails.
    const newer = gatedDb(reconcileBatch, "fail");
    await expect(refreshBilling(newer.env, userId, new PolarApi(newer.env))).rejects.toThrow(
      /refused/,
    );

    // The older reconcile is now a no-op: the debt stays, and so does the event.
    older.held[0]!();
    expect(await olderRefresh).toBe("committed");
    expect(servingState(userId, machineId).owed).not.toBeNull();
    expect(events().find((event) => event.id === "msg_older")!.processed_at).toBeNull();
  });

  it("marks only the events at or below its watermark as processed", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    subscription(userId);
    const insert = (id: string) =>
      run(
        `INSERT INTO webhook_events (id, type, user_id, received_at, next_attempt_at)
         VALUES (?, 'subscription.updated', ?, 1, 1)`,
        id,
        userId,
      );
    insert("before");
    const held = holdListings();
    const refreshing = refresh(userId);
    await vi.waitFor(() => expect(held).toHaveLength(1));
    // Arrives while the refresh reads Polar.
    insert("during");
    held[0]!();
    expect(await refreshing).toBe("committed");

    const byId = Object.fromEntries(events().map((event) => [event.id, event.processed_at]));
    expect(byId.before).not.toBeNull();
    expect(byId.during).toBeNull();
  });

  /** The plan `/v1/me` reports once Polar holds `fields`. */
  async function planFor(fields: Raw) {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    subscription(userId, fields);
    await refresh(userId);
    return plan(token);
  }

  it.each([
    [
      "trialing",
      {
        status: "trialing",
        trial_start: iso(Date.now() - DAY),
        trial_end: iso(Date.now() + 6 * DAY),
      },
      "trialing",
      true,
    ],
    ["active", {}, "active", true],
    [
      "canceling until the period end",
      { cancel_at_period_end: true, ends_at: iso(Date.now() + DAY) },
      "canceling",
      true,
    ],
    [
      "canceled with an end ahead",
      { status: "canceled", ends_at: iso(Date.now() + DAY) },
      "canceling",
      true,
    ],
    [
      "past_due within the grace",
      { status: "past_due", past_due_at: iso(Date.now() - DAY) },
      "past_due",
      true,
    ],
    [
      "past_due after the grace",
      { status: "past_due", past_due_at: iso(Date.now() - GRACE - 60_000) },
      "past_due",
      false,
    ],
    [
      "revoked",
      { status: "canceled", ended_at: iso(Date.now() - 1000), ends_at: iso(Date.now() - 1000) },
      "none",
      false,
    ],
    [
      "ended at the period end",
      { cancel_at_period_end: true, current_period_end: iso(Date.now() - 1000) },
      "none",
      false,
    ],
    ["unpaid", { status: "unpaid" }, "none", false],
    ["paused", { status: "paused" }, "none", false],
    ["incomplete", { status: "incomplete" }, "none", false],
    ["incomplete_expired", { status: "incomplete_expired" }, "none", false],
    ["another product", { product_id: "prod-other" }, "none", false],
  ])("maps %s", async (_name, fields, status, entitled) => {
    expect(await planFor(fields)).toMatchObject({ status, entitled });
  });

  it("gives no plan without any subscription", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    expect(await refresh(userId)).toBe("committed");
    expect(await plan(token)).toMatchObject({ status: "none", entitled: false });
  });

  it("keeps a plan whose order was refunded: only a revocation ends it", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    // Polar keeps the subscription active after refunding an order.
    subscription(userId);
    await refresh(userId);
    expect(await plan(token)).toMatchObject({ status: "active", entitled: true });
    // The refresh reads subscriptions only: orders and refunds play no part.
    expect(polarCalls("GET", "/v1/orders/")).toEqual([]);
    expect(polarCalls("GET", "/v1/refunds/")).toEqual([]);
  });

  it("keeps past_due entitled until 7 days after Polar's past_due_at", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    const sub = subscription(userId, { status: "past_due" });

    Object.assign(sub, { past_due_at: iso(Date.now() - GRACE + 60_000) });
    await refresh(userId);
    expect(await plan(token)).toMatchObject({ status: "past_due", entitled: true });

    Object.assign(sub, { past_due_at: iso(Date.now() - GRACE - 60_000) });
    await refresh(userId);
    expect(await plan(token)).toMatchObject({ status: "past_due", entitled: false });

    // Without past_due_at, the grace has no start.
    Object.assign(sub, { past_due_at: null });
    await refresh(userId);
    expect(await plan(token)).toMatchObject({ status: "past_due", entitled: false });
  });

  it("prefers an entitled subscription to a newer one that is not", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    subscription(userId, {
      product_id: YEARLY,
      recurring_interval: "year",
      started_at: iso(Date.now() - 100 * DAY),
    });
    subscription(userId, { status: "incomplete_expired", started_at: iso(Date.now()) });
    await refresh(userId);
    expect(await plan(token)).toMatchObject({ status: "active", interval: "year", entitled: true });
  });

  it("records the trial history, so later checkouts have no trial", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    // A past trial, long ended.
    subscription(userId, {
      status: "canceled",
      trial_start: iso(Date.now() - 60 * DAY),
      trial_end: iso(Date.now() - 53 * DAY),
      ended_at: iso(Date.now() - 53 * DAY),
    });
    expect(await plan(token)).toMatchObject({ trialAvailable: true });
    await refresh(userId);
    expect(await plan(token)).toMatchObject({ status: "none", trialAvailable: false });

    const response = await api(context, "POST", "/v1/billing/checkout", {
      token,
      body: { product: "pro_monthly" },
    });
    expect(response.status).toBe(200);
    const [call] = polarCalls("POST", "/v1/checkouts/");
    expect(JSON.parse(call!.body!)).toMatchObject({ allow_trial: false });
  });

  it("runs the serving reconcile when the entitlement changes", async () => {
    const { token, userId, machineId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    const sub = subscription(userId);
    await deliver(subscriptionEvent(userId, "subscription.created"));
    await settle(context);
    const created = await api(context, "POST", "/v1/public-urls", {
      token,
      body: {
        localDomain: "journal.localify",
        slug: "journal-billing",
        protection: { mode: "public" },
      },
    });
    expect(created.status).toBe(201);
    await settle(context);
    await serve(context, token);
    expect(
      get<{ tunnel_state: string }>("SELECT tunnel_state FROM machines WHERE id = ?", machineId)
        .tunnel_state,
    ).toBe("ready");

    // Polar revokes the plan: the URL stops serving and the machine rotates.
    Object.assign(sub, { status: "canceled", ended_at: iso(Date.now()) });
    await deliver(subscriptionEvent(userId, "subscription.revoked"));
    await settle(context);
    expect(
      get<{ suspended_at: number | null }>(
        "SELECT suspended_at FROM public_urls WHERE user_id = ?",
        userId,
      ).suspended_at,
    ).not.toBeNull();
    expect(
      get<{ rotate_requested_at: number | null }>(
        "SELECT rotate_requested_at FROM machines WHERE id = ?",
        machineId,
      ).rotate_requested_at,
    ).not.toBeNull();

    // Polar restores it: the URL serves again.
    Object.assign(sub, { status: "active", ended_at: null });
    await deliver(subscriptionEvent(userId, "subscription.uncanceled"));
    await settle(context);
    expect(
      get<{ suspended_at: number | null }>(
        "SELECT suspended_at FROM public_urls WHERE user_id = ?",
        userId,
      ).suspended_at,
    ).toBeNull();
  });

  it("leaves the serving state alone when the entitlement does not change", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    subscription(userId);
    await refresh(userId);
    const before = context.queries.count;
    await refresh(userId);
    // The two batches and the mark of no events: no reconcile reads.
    expect(context.queries.count - before).toBe(2);
  });
});

// --- Duplicate subscriptions ---

describe("duplicate Pro subscriptions", () => {
  type Duplicate = { revoked_at: number | null; attempts: number; next_attempt_at: number | null };
  function duplicateRow(id: string): Duplicate {
    return get<Duplicate>(
      "SELECT revoked_at, attempts, next_attempt_at FROM duplicate_subscriptions WHERE subscription_id = ?",
      id,
    );
  }

  /** No refund is ever made, or even looked at: that is an operator's. */
  function expectNoRefundWork(): void {
    expect(polarCalls("POST", "/v1/refunds/")).toEqual([]);
    expect(polarCalls("GET", "/v1/refunds/")).toEqual([]);
    expect(polarCalls("GET", "/v1/orders/")).toEqual([]);
  }

  it("keep the one ending latest, revoke the other, and never refund it", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    const monthly = subscription(userId, { current_period_end: iso(Date.now() + 20 * DAY) });
    const yearly = subscription(userId, {
      product_id: YEARLY,
      recurring_interval: "year",
      current_period_end: iso(Date.now() + 360 * DAY),
    });
    const alerts = vi.spyOn(console, "error").mockImplementation(() => {});

    await refresh(userId);
    expect(billingRow(userId)).toMatchObject({
      pro_subscription_id: yearly.id,
      pro_interval: "year",
    });
    expect(alerts.mock.calls.some((call) => String(call[0]).includes(String(monthly.id)))).toBe(
      true,
    );
    expect(duplicateRow(String(monthly.id))).toEqual({
      revoked_at: null,
      attempts: 0,
      next_attempt_at: null,
    });
    // Recorded before any call: the refresh itself revokes nothing.
    expect(polar.revoked).toEqual([]);

    alerts.mockClear();
    await runBillingJobs(context.env, Date.now());
    expect(polar.revoked).toEqual([monthly.id]);
    expect(duplicateRow(String(monthly.id)).revoked_at).not.toBeNull();
    // The revocation alerts again, pointing at the listing to refund from.
    expect(
      alerts.mock.calls.some((call) => String(call[0]).includes("pnpm cloud:admin duplicates")),
    ).toBe(true);

    // Revoked, it is no duplicate any more, and the kept one still serves.
    await refresh(userId);
    expect(await plan(token)).toMatchObject({ status: "active", interval: "year", entitled: true });
    await runBillingJobs(context.env, Date.now() + DAY);
    expect(polar.revoked).toHaveLength(1);
    expectNoRefundWork();
  });

  it("retry a failed revocation after 5 minutes, doubling", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const duplicate = subscription(userId, { current_period_end: iso(Date.now() + 10 * DAY) });
    subscription(userId, { current_period_end: iso(Date.now() + 20 * DAY) });
    vi.spyOn(console, "error").mockImplementation(() => {});
    await refresh(userId);
    const revoke = context.routes.get(`DELETE ${POLAR}/v1/subscriptions/`)!;
    context.routes.set(`DELETE ${POLAR}/v1/subscriptions/`, () =>
      jsonResponse({ error: "SubscriptionLocked" }, 409),
    );

    const now = Date.now();
    await runBillingJobs(context.env, now);
    const id = String(duplicate.id);
    expect(duplicateRow(id)).toEqual({
      revoked_at: null,
      attempts: 1,
      next_attempt_at: now + 5 * 60 * 1000,
    });
    // Not due yet: left alone.
    await runBillingJobs(context.env, now + 60_000);
    expect(duplicateRow(id).attempts).toBe(1);
    await runBillingJobs(context.env, now + 5 * 60 * 1000);
    expect(duplicateRow(id)).toMatchObject({
      attempts: 2,
      next_attempt_at: now + 15 * 60 * 1000,
    });

    context.routes.set(`DELETE ${POLAR}/v1/subscriptions/`, revoke);
    await runBillingJobs(context.env, now + 15 * 60 * 1000);
    expect(duplicateRow(id)).toMatchObject({ attempts: 0, next_attempt_at: null });
    expect(duplicateRow(id).revoked_at).not.toBeNull();
    expectNoRefundWork();
  });

  it("revoke a new duplicate first, however many old ones keep failing or refreshes wait", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const now = Date.now();
    // 20 old duplicates Polar no longer knows, all due again.
    for (let i = 0; i < 20; i++) {
      run(
        `INSERT INTO duplicate_subscriptions (subscription_id, user_id, attempts, next_attempt_at, created_at)
         VALUES (?, ?, 3, ?, ?)`,
        `old-${String(i).padStart(2, "0")}`,
        userId,
        now - 1000 + i,
        i,
      );
    }
    // And a refresh queue full enough to spend the refreshes' whole budget.
    for (let i = 0; i < 30; i++) {
      addUser(`waiting-${i}`, 1000 + i);
      run(
        "INSERT INTO billing_revisions (user_id, last, check_due_at) VALUES (?, 0, 1)",
        `waiting-${i}`,
      );
    }
    const fresh = subscription(userId, { current_period_end: iso(now + 10 * DAY) });
    subscription(userId, { current_period_end: iso(now + 20 * DAY) });
    vi.spyOn(console, "error").mockImplementation(() => {});
    await refresh(userId);

    const attempted = () =>
      polarCalls("DELETE", "/v1/subscriptions/").map((call) =>
        call.url.slice(`${POLAR}/v1/subscriptions/`.length),
      );
    await runBillingJobs(context.env, now);
    const first = attempted();
    // The new one first, then the old ones due longest.
    expect(first[0]).toBe(fresh.id);
    expect(first).toHaveLength(10);
    expect(duplicateRow(String(fresh.id)).revoked_at).not.toBeNull();

    // The next run goes on with the old ones not tried yet.
    context.calls.length = 0;
    await runBillingJobs(context.env, now);
    const second = attempted();
    expect(second).toHaveLength(10);
    expect(second.filter((id) => first.includes(id))).toEqual([]);
    expectNoRefundWork();
  });

  it("wait out Polar's rate limit, which is no failure", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const duplicate = subscription(userId, { current_period_end: iso(Date.now() + 10 * DAY) });
    subscription(userId, { current_period_end: iso(Date.now() + 20 * DAY) });
    vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(console, "warn").mockImplementation(() => {});
    await refresh(userId);
    context.routes.set(
      `DELETE ${POLAR}/v1/subscriptions/`,
      () =>
        new Response(JSON.stringify({ error: "TooManyRequests" }), {
          status: 429,
          headers: { "retry-after": "30" },
        }),
    );
    const now = Date.now();
    await runBillingJobs(context.env, now);
    expect(duplicateRow(String(duplicate.id))).toEqual({
      revoked_at: null,
      attempts: 0,
      next_attempt_at: now + 30_000,
    });
  });
});

// --- Checkout and portal ---

describe("checkout", () => {
  async function checkout(
    token: string,
    product = "pro_monthly",
    headers: Record<string, string> = {},
  ) {
    const response = await api(context, "POST", "/v1/billing/checkout", {
      token,
      body: { product },
      headers,
    });
    return { status: response.status, body: (await response.json()) as Record<string, string> };
  }

  it("creates a Pro checkout for both intervals, the requested one first", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    const page = await checkout(token, "pro_yearly", { "cf-connecting-ip": "203.0.113.7" });
    expect(page).toEqual({ status: 200, body: { kind: "checkout", url: `${POLAR}/checkout/1` } });

    const [call] = polarCalls("POST", "/v1/checkouts/");
    expect(call!.headers.get("polar-version")).toBe("2026-10");
    expect(JSON.parse(call!.body!)).toEqual({
      products: [YEARLY, MONTHLY],
      external_customer_id: userId,
      allow_trial: true,
      metadata: { userId },
      success_url: "https://localify.dev/cloud/subscribed?checkout_id={CHECKOUT_ID}",
      return_url: "https://localify.dev/pricing",
      // Polar picks the currency from the buyer's address, not Cloudflare's.
      customer_ip_address: "203.0.113.7",
    });
  });

  it("sends no address when the request has none", async () => {
    const { token } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    await checkout(token);
    const [call] = polarCalls("POST", "/v1/checkouts/");
    expect(JSON.parse(call!.body!)).not.toHaveProperty("customer_ip_address");
  });

  it("refuses any other product", async () => {
    const { token } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    for (const product of ["addon_extra", "prod-monthly", ""]) {
      expect((await checkout(token, product)).body.error).toBe("invalid_product");
    }
    expect(polarCalls("POST", "/v1/checkouts/")).toEqual([]);
  });

  it("keeps one pending checkout per user, under concurrency and for either interval", async () => {
    const { token } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const held: (() => void)[] = [];
    const create = polar.checkout;
    polar.checkout = (body) =>
      new Promise((resolve) => held.push(() => resolve(create(body) as Response)));

    const first = checkout(token);
    await vi.waitFor(() => expect(held).toHaveLength(1));
    const second = await checkout(token, "pro_yearly");
    expect(second).toMatchObject({ status: 409, body: { error: "checkout_in_progress" } });
    held[0]!();
    expect(await first).toEqual({
      status: 200,
      body: { kind: "checkout", url: `${POLAR}/checkout/1` },
    });

    // Reused, whichever interval, while it is open.
    expect((await checkout(token, "pro_yearly")).body.url).toBe(`${POLAR}/checkout/1`);
    expect(polarCalls("POST", "/v1/checkouts/")).toHaveLength(1);
  });

  it("returns the checkout its row stores when another request took the row over", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const held: (() => void)[] = [];
    const create = polar.checkout;
    polar.checkout = (body) =>
      new Promise((resolve) => held.push(() => resolve(create(body) as Response)));
    vi.spyOn(console, "warn").mockImplementation(() => {});

    const start = Date.now();
    const a = createCheckout(context.env, userId, { kind: "pro", interval: "month" }, null, start);
    await vi.waitFor(() => expect(held).toHaveLength(1));
    // A's call takes more than 30 seconds: B takes the row over.
    const b = createCheckout(
      context.env,
      userId,
      { kind: "pro", interval: "year" },
      null,
      start + 31_000,
    );
    await vi.waitFor(() => expect(held).toHaveLength(2));

    held[1]!();
    expect(await b).toEqual({ kind: "checkout", url: `${POLAR}/checkout/1` });
    held[0]!();
    // A's own checkout, the second one Polar made, is never given out.
    expect(await a).toEqual({ kind: "checkout", url: `${POLAR}/checkout/1` });
    expect(get("SELECT checkout_id, url FROM checkouts WHERE user_id = ?", userId)).toEqual({
      checkout_id: "checkout-1",
      url: `${POLAR}/checkout/1`,
    });
  });

  it("answers 409 to a request that lost its row before the winner is ready", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const held: (() => void)[] = [];
    const create = polar.checkout;
    polar.checkout = (body) =>
      new Promise((resolve) => held.push(() => resolve(create(body) as Response)));
    vi.spyOn(console, "warn").mockImplementation(() => {});

    const start = Date.now();
    const a = createCheckout(context.env, userId, { kind: "pro", interval: "month" }, null, start);
    await vi.waitFor(() => expect(held).toHaveLength(1));
    const b = createCheckout(
      context.env,
      userId,
      { kind: "pro", interval: "month" },
      null,
      start + 31_000,
    );
    await vi.waitFor(() => expect(held).toHaveLength(2));

    held[0]!();
    await expect(a).rejects.toMatchObject({ status: 409, code: "checkout_in_progress" });
    held[1]!();
    expect(await b).toEqual({ kind: "checkout", url: `${POLAR}/checkout/2` });
  });

  it("replaces a checkout about to expire, and one stuck creating", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    await checkout(token);
    run("UPDATE checkouts SET expires_at = ? WHERE user_id = ?", Date.now() + 60_000, userId);
    expect((await checkout(token)).body.url).toBe(`${POLAR}/checkout/2`);

    run(
      "UPDATE checkouts SET state = 'creating', created_at = ? WHERE user_id = ?",
      Date.now() - 60_000,
      userId,
    );
    expect((await checkout(token)).body.url).toBe(`${POLAR}/checkout/3`);
  });

  it("lets the next request try again when Polar fails", async () => {
    const { token } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const create = polar.checkout;
    polar.checkout = () => jsonResponse({ error: "InternalServerError" }, 500);
    vi.spyOn(console, "error").mockImplementation(() => {});
    expect(await checkout(token)).toMatchObject({
      status: 502,
      body: { error: "billing_unavailable" },
    });
    polar.checkout = create;
    expect((await checkout(token)).status).toBe(200);
  });

  it("gives a user who already has Pro the portal, never a second checkout", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    subscription(userId);
    await refresh(userId);
    expect(await checkout(token, "pro_yearly")).toEqual({
      status: 200,
      body: { kind: "portal", url: `${POLAR}/portal/${userId}` },
    });
    expect(polarCalls("POST", "/v1/checkouts/")).toEqual([]);
    const [session] = polarCalls("POST", "/v1/customer-sessions/");
    expect(JSON.parse(session!.body!)).toEqual({
      external_customer_id: userId,
      return_url: "https://localify.dev/cloud",
    });
  });

  it("grants nothing by itself: access starts with a refresh", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    await checkout(token);
    expect(await plan(token)).toMatchObject({ entitled: false });
    subscription(userId, {
      status: "trialing",
      trial_start: iso(Date.now()),
      trial_end: iso(Date.now() + 7 * DAY),
    });
    await deliver(subscriptionEvent(userId, "subscription.created"));
    await settle(context);
    expect(await plan(token)).toMatchObject({
      status: "trialing",
      entitled: true,
      trialAvailable: false,
    });
  });
});

describe("the portal", () => {
  it("answers no_billing for a user who never checked out", async () => {
    const { token } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const response = await api(context, "POST", "/v1/billing/portal", { token });
    expect(response.status).toBe(404);
    expect(await response.json()).toMatchObject({ error: "no_billing" });
    expect(polarCalls("POST", "/v1/customer-sessions/")).toEqual([]);
  });

  it("opens a customer session for a customer", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    subscription(userId);
    await refresh(userId);
    const response = await api(context, "POST", "/v1/billing/portal", { token });
    expect(await response.json()).toEqual({ url: `${POLAR}/portal/${userId}` });
  });

  it("answers no_billing when Polar knows no such customer", async () => {
    const { token } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    await api(context, "POST", "/v1/billing/checkout", { token, body: { product: "pro_monthly" } });
    context.routes.set(`POST ${POLAR}/v1/customer-sessions/`, () =>
      jsonResponse({ error: "ResourceNotFound" }, 404),
    );
    const response = await api(context, "POST", "/v1/billing/portal", { token });
    expect(response.status).toBe(404);
  });
});

// --- The jobs ---

describe("the reconciliation", () => {
  it("refreshes due users oldest first, within the budget of each run", async () => {
    for (let i = 0; i < 40; i++) {
      const id = `user-${String(i).padStart(2, "0")}`;
      addUser(id, 100 + i);
      run(
        "INSERT INTO billing_revisions (user_id, last, check_due_at) VALUES (?, 0, ?)",
        id,
        1000 + i,
      );
      // Each one gains Pro, so each refresh runs a serving reconcile too.
      subscription(id);
    }
    const before = context.queries.count;
    await runBillingJobs(context.env, Date.now());
    const refreshed = (
      context.sqlite
        .prepare(
          "SELECT user_id FROM billing_revisions WHERE check_due_at IS NULL ORDER BY user_id",
        )
        .all() as { user_id: string }[]
    ).map((row) => row.user_id);
    // The oldest first, as many as the run allows.
    expect(refreshed.length).toBeGreaterThan(0);
    expect(refreshed.length).toBeLessThan(40);
    expect(refreshed).toEqual(
      Array.from({ length: refreshed.length }, (_, i) => `user-${String(i).padStart(2, "0")}`),
    );
    expect(context.queries.count - before).toBeLessThanOrEqual(80);
    expect(polarCalls("GET", "/v1/subscriptions/").length).toBeLessThanOrEqual(40);

    // The next runs take the rest.
    for (let run = 0; run < 5; run++) await runBillingJobs(context.env, Date.now());
    expect(
      get("SELECT COUNT(*) AS n FROM billing_revisions WHERE check_due_at IS NOT NULL"),
    ).toEqual({ n: 0 });
  });

  it("sends a user whose refresh fails to the back of the queue", async () => {
    addUser("user-a", 100);
    addUser("user-b", 101);
    run(
      "INSERT INTO billing_revisions (user_id, last, check_due_at) VALUES ('user-a', 0, 1), ('user-b', 0, 2)",
    );
    context.routes.set(`GET ${POLAR}/v1/subscriptions/`, (_body, url) =>
      url.includes("user-a") ? jsonResponse({}, 500) : page([], url),
    );
    vi.spyOn(console, "error").mockImplementation(() => {});
    const now = Date.now();
    await runBillingJobs(context.env, now);
    expect(get("SELECT check_due_at FROM billing_revisions WHERE user_id = 'user-a'")).toEqual({
      check_due_at: now,
    });
    expect(get("SELECT check_due_at FROM billing_revisions WHERE user_id = 'user-b'")).toEqual({
      check_due_at: null,
    });
  });

  it("queues every user with a snapshot, a checkout or a Polar subscription, once a day", async () => {
    for (const [id, githubId] of [
      ["with-billing", 100],
      ["with-checkout", 101],
      ["in-polar", 102],
      ["nobody", 103],
    ] as const) {
      addUser(id, githubId);
    }
    run("INSERT INTO billing (user_id, revision, refreshed_at) VALUES ('with-billing', 1, 1)");
    run(
      "INSERT INTO checkouts (user_id, kind, state, created_at) VALUES ('with-checkout', 'pro', 'open', 1)",
    );
    subscription("in-polar");
    subscription("not-a-user");
    // Queued earlier: keeps its place.
    run(
      "INSERT INTO billing_revisions (user_id, last, check_due_at) VALUES ('with-billing', 3, 5)",
    );

    const now = Date.now();
    await reconcileBilling(context.env, now);
    const due = Object.fromEntries(
      (
        context.sqlite.prepare("SELECT user_id, check_due_at FROM billing_revisions").all() as {
          user_id: string;
          check_due_at: number | null;
        }[]
      ).map((row) => [row.user_id, row.check_due_at]),
    );
    expect(due).toEqual({ "with-billing": 5, "with-checkout": now, "in-polar": now });

    // The 5-minute job then refreshes them.
    await runBillingJobs(context.env, now);
    expect(billingRow("in-polar")).toMatchObject({ pro_status: "active" });
    expect(
      get("SELECT COUNT(*) AS n FROM billing_revisions WHERE check_due_at IS NOT NULL"),
    ).toEqual({ n: 0 });
  });

  it("reads the organization's subscriptions within its budget, a different window each day", async () => {
    const MAX_PAGE = 50;
    const read: number[][] = [];
    let day: number[] = [];
    context.routes.set(`GET ${POLAR}/v1/subscriptions/`, (_body, url) => {
      const number = Number(new URL(url).searchParams.get("page"));
      day.push(number);
      return jsonResponse({
        items: [
          {
            id: `sub-of-page-${number}`,
            status: "active",
            product_id: MONTHLY,
            customer: { external_id: `page-${number}` },
          },
        ],
        pagination: { total_count: MAX_PAGE * 100, max_page: MAX_PAGE },
      });
    });
    const today = Math.floor(Date.now() / DAY) * DAY;
    for (let offset = 0; offset < 3; offset++) {
      day = [];
      await reconcileBilling(context.env, today + offset * DAY + 3 * 60 * 60 * 1000);
      read.push(day);
    }
    for (const pages of read) {
      expect(pages).toHaveLength(20);
      expect(pages[0]).toBe(1);
      expect(new Set(pages).size).toBe(20);
    }
    expect(read[0]).not.toEqual(read[1]);
    // Three days cover every page.
    expect(new Set(read.flat()).size).toBe(MAX_PAGE);
  });

  it("purges processed events after 30 days, never pending ones", async () => {
    const now = Date.now();
    run(
      `INSERT INTO webhook_events (id, type, received_at, processed_at) VALUES
         ('old', 't', 1, ?), ('recent', 't', 1, ?)`,
      now - 31 * DAY,
      now - DAY,
    );
    run(`INSERT INTO webhook_events (id, type, received_at) VALUES ('pending', 't', 1)`);
    await reconcileBilling(context.env, now);
    expect(events().map((event) => event.id)).toEqual(["recent", "pending"]);
  });
});

describe("cloud:admin refresh", () => {
  it("is carried out by the next 5-minute run", async () => {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    subscription(userId);
    // What `pnpm cloud:admin refresh` writes.
    run(
      "INSERT INTO billing_revisions (user_id, last, check_due_at) VALUES (?, 0, ?)",
      userId,
      Date.now(),
    );
    await runBillingJobs(context.env, Date.now());
    expect(billingRow(userId)).toMatchObject({ pro_status: "active", revision: 1 });
    expect(get("SELECT check_due_at FROM billing_revisions WHERE user_id = ?", userId)).toEqual({
      check_due_at: null,
    });
  });

  it("makes no Polar call when nothing is due", async () => {
    await runBillingJobs(context.env, Date.now());
    expect(context.calls).toEqual([]);
  });
});

// --- Add-ons ---

describe("add-ons", () => {
  const MACHINE = "prod-addon-machine";
  const URLS = "prod-addon-urls";

  beforeEach(() => {
    Object.assign(context.env, {
      ADDONS: "on",
      POLAR_PRODUCT_ADDON_MACHINE: MACHINE,
      POLAR_PRODUCT_ADDON_URLS: URLS,
    });
  });

  async function buy(
    token: string,
    product = "addon_machine",
    units: unknown = 2,
    headers: Record<string, string> = {},
  ) {
    const response = await api(context, "POST", "/v1/billing/checkout", {
      token,
      body: { product, units },
      headers,
    });
    return { status: response.status, body: (await response.json()) as Record<string, unknown> };
  }

  /** A monthly add-on subscription of `userId`, active, with `units`. */
  function addon(userId: string, product: string, units: number, fields: Raw = {}): Raw {
    return subscription(userId, { product_id: product, units, ...fields });
  }

  /** A user whose Pro plan is active, refreshed. */
  async function proUser(githubId = 1) {
    const user = await signIn(context, `user-${githubId}`, githubId, "laptop", {
      entitled: false,
    });
    subscription(user.userId);
    await refresh(user.userId);
    return user;
  }

  function cancellations(): { subscription_id: string; canceled_at: number | null }[] {
    return context.sqlite
      .prepare("SELECT subscription_id, canceled_at FROM addon_cancellations ORDER BY 1")
      .all() as never;
  }

  describe("checkout", () => {
    it("is refused with 409 while ADDONS is off, before asking Polar anything", async () => {
      const { token } = await proUser();
      context.env.ADDONS = "off";
      const before = context.calls.length;
      for (const product of ["addon_machine", "addon_urls"]) {
        expect(await buy(token, product)).toMatchObject({
          status: 409,
          body: { error: "addons_unavailable" },
        });
      }
      expect(context.calls.length).toBe(before);
    });

    it("needs an entitled Pro plan: none, an override or a lapsed one answers 402", async () => {
      context.env.ALLOWED_GITHUB_IDS = "1, 2, 3";
      const none = await signIn(context, "none", 1, "laptop", { entitled: false });
      const team = await signIn(context, "team", 2, "laptop");
      const lapsed = await signIn(context, "lapsed", 3, "laptop", { entitled: false });
      subscription(lapsed.userId, { status: "canceled", ended_at: iso(Date.now() - DAY) });
      await refresh(lapsed.userId);

      for (const { token } of [none, team, lapsed]) {
        expect(await buy(token)).toEqual({
          status: 402,
          body: {
            error: "subscription_required",
            message: expect.stringContaining("localify subscribe"),
            upgrade: { kind: "pro", command: "localify subscribe" },
          },
        });
      }
      expect(polarCalls("POST", "/v1/checkouts/")).toEqual([]);
    });

    it("sells the quantity asked of the configured product, without a trial", async () => {
      const { token, userId } = await proUser();
      const page = await buy(token, "addon_urls", 3, { "cf-connecting-ip": "203.0.113.7" });
      expect(page).toEqual({ status: 200, body: { kind: "checkout", url: `${POLAR}/checkout/1` } });
      const [call] = polarCalls("POST", "/v1/checkouts/");
      expect(JSON.parse(call!.body!)).toEqual({
        products: [URLS],
        external_customer_id: userId,
        allow_trial: false,
        metadata: { userId },
        success_url: "https://localify.dev/cloud/subscribed?checkout_id={CHECKOUT_ID}",
        return_url: "https://localify.dev/pricing",
        customer_ip_address: "203.0.113.7",
        units: 3,
      });
    });

    it("refuses a quantity that is not a whole number from 1 to 20", async () => {
      const { token } = await proUser();
      for (const units of [0, 21, 1.5, "2", null]) {
        expect((await buy(token, "addon_machine", units)).body.error, String(units)).toBe(
          "invalid_units",
        );
      }
      expect(polarCalls("POST", "/v1/checkouts/")).toEqual([]);
    });

    it("reuses the pending checkout for the same quantity, and replaces it for another", async () => {
      const { token, userId } = await proUser();
      const first = await buy(token, "addon_machine", 2);
      expect(await buy(token, "addon_machine", 2)).toEqual(first);
      expect(polarCalls("POST", "/v1/checkouts/")).toHaveLength(1);

      const other = await buy(token, "addon_machine", 3);
      expect(other.body.url).toBe(`${POLAR}/checkout/2`);
      expect(
        get("SELECT units FROM checkouts WHERE user_id = ? AND kind = 'addon_machine'", userId),
      ).toEqual({ units: 3 });
      // Each add-on keeps a row of its own.
      expect((await buy(token, "addon_urls", 1)).body.url).toBe(`${POLAR}/checkout/3`);
    });

    it("opens the portal when an add-on of that kind exists, never a second checkout", async () => {
      const { token, userId } = await proUser();
      addon(userId, MACHINE, 1);
      await refresh(userId);
      expect(await buy(token, "addon_machine", 1)).toEqual({
        status: 200,
        body: { kind: "portal", url: `${POLAR}/portal/${userId}` },
      });
      // One whose webhook has not arrived yet is found in Polar.
      addon(userId, URLS, 1);
      expect(await buy(token, "addon_urls", 2)).toEqual({
        status: 200,
        body: { kind: "portal", url: `${POLAR}/portal/${userId}` },
      });
      expect(polarCalls("POST", "/v1/checkouts/")).toEqual([]);
    });
  });

  describe("entitlement", () => {
    it("adds each unit while Pro is entitled, and follows a unit change from a webhook", async () => {
      const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
        entitled: false,
      });
      subscription(userId);
      const machines = addon(userId, MACHINE, 2);
      const urls = addon(userId, URLS, 1);
      await deliver(subscriptionEvent(userId, "subscription.created"));
      await settle(context);
      expect(await plan(token)).toMatchObject({
        limits: { servingMachines: 4, publicUrls: 20, remoteAccess: true },
        addons: { machines: 2, urlPacks: 1 },
      });

      machines.units = 3;
      await deliver(subscriptionEvent(userId));
      await settle(context);
      expect(await plan(token)).toMatchObject({
        limits: { servingMachines: 5, publicUrls: 20 },
        addons: { machines: 3, urlPacks: 1 },
      });

      // Set to cancel, it counts until its period ends; ended, it does not.
      urls.cancel_at_period_end = true;
      await deliver(subscriptionEvent(userId));
      await settle(context);
      expect(await plan(token)).toMatchObject({ limits: { publicUrls: 20 } });
      Object.assign(urls, { status: "canceled", ended_at: iso(Date.now()) });
      await deliver(subscriptionEvent(userId));
      await settle(context);
      expect(await plan(token)).toMatchObject({
        limits: { servingMachines: 5, publicUrls: 10 },
        addons: { machines: 3, urlPacks: 0 },
      });
    });

    it("stops counting an add-on when its period ends, with no refresh and no Polar call", async () => {
      const { token, userId } = await proUser();
      const now = Date.now();
      addon(userId, MACHINE, 2, {
        cancel_at_period_end: true,
        current_period_end: iso(now + 1000),
      });
      await refresh(userId);
      expect(await plan(token)).toMatchObject({ limits: { servingMachines: 4 } });

      const calls = context.calls.length;
      vi.spyOn(Date, "now").mockReturnValue(now + 60 * 60 * 1000);
      expect(await plan(token)).toMatchObject({
        limits: { servingMachines: 2, publicUrls: 10 },
        addons: { machines: 0, urlPacks: 0 },
      });
      expect(context.calls.length).toBe(calls);
    });

    it("counts add-ons even while they are not on sale", async () => {
      context.env.ADDONS = "off";
      const { token, userId } = await proUser();
      addon(userId, MACHINE, 1);
      await refresh(userId);
      expect(await plan(token)).toMatchObject({ limits: { servingMachines: 3 } });
    });

    it("gives nothing for add-ons without an entitled Pro plan, and ignores them under an override", async () => {
      const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
        entitled: false,
      });
      subscription(userId, { status: "canceled", ended_at: iso(Date.now() - DAY) });
      addon(userId, MACHINE, 2);
      await refresh(userId);
      expect(await plan(token)).toMatchObject({
        entitled: false,
        limits: { servingMachines: 0, publicUrls: 0 },
      });
      run(
        `INSERT INTO plan_overrides (user_id, serving_machines, public_urls, remote_access, reason, created_at)
         VALUES (?, 5, 10, 1, 'team', 1)`,
        userId,
      );
      expect(await plan(token)).toMatchObject({
        status: "override",
        limits: { servingMachines: 5, publicUrls: 10 },
      });
    });
  });

  describe("when Pro lapses", () => {
    /** Every body the Worker sent to change a subscription. */
    function patches(): { cancel_at_period_end: boolean }[] {
      return polarCalls("PATCH", "/v1/subscriptions/").map(
        (call) => JSON.parse(call.body!) as { cancel_at_period_end: boolean },
      );
    }

    it("are canceled at period end by the 5-minute job, once, never refunded", async () => {
      const { userId } = await proUser();
      const pro = polar.subscriptions[0]!;
      const machine = addon(userId, MACHINE, 2);
      await refresh(userId);
      expect(cancellations()).toEqual([]);

      // After the past_due grace too, Polar may still recover the card.
      Object.assign(pro, { status: "past_due", past_due_at: iso(Date.now() - 8 * DAY) });
      await refresh(userId);
      expect(cancellations()).toEqual([]);

      Object.assign(pro, { status: "canceled", ended_at: iso(Date.now()) });
      await refresh(userId);
      expect(cancellations()).toEqual([{ subscription_id: machine.id, canceled_at: null }]);
      // Recorded by the refresh, canceled by the job.
      expect(polar.canceled).toEqual([]);

      await runBillingJobs(context.env, Date.now());
      expect(polar.canceled).toEqual([{ id: machine.id, body: { cancel_at_period_end: true } }]);
      expect(cancellations()[0]!.canceled_at).not.toBeNull();
      expect(polar.revoked).toEqual([]);

      // Idempotent: another run cancels nothing again, and the next refresh,
      // which reads the subscription as canceling, drops the record.
      await runBillingJobs(context.env, Date.now());
      await refresh(userId);
      expect(cancellations()).toEqual([]);
      await runBillingJobs(context.env, Date.now());
      expect(polar.canceled).toHaveLength(1);
    });

    it("counts a cancellation Polar already applied as done", async () => {
      const { userId } = await proUser();
      const machine = addon(userId, MACHINE, 1, { cancel_at_period_end: true });
      Object.assign(polar.subscriptions[0]!, { status: "canceled", ended_at: iso(Date.now()) });
      await refresh(userId);
      // A record from before Polar applied it, at the snapshot's revision.
      run(
        `INSERT INTO addon_cancellations (subscription_id, user_id, revision, created_at)
         VALUES (?, ?, (SELECT revision FROM billing), 1)`,
        String(machine.id),
        userId,
      );
      vi.spyOn(console, "warn").mockImplementation(() => {});
      await runBillingJobs(context.env, Date.now());
      expect(polarCalls("PATCH", "/v1/subscriptions/")).toHaveLength(1);
      expect(get("SELECT state FROM addon_cancellations")).toEqual({ state: "canceled" });
    });

    it("cancels one set back to renew in the portal while Pro is still lapsed", async () => {
      const { userId } = await proUser();
      Object.assign(polar.subscriptions[0]!, { status: "canceled", ended_at: iso(Date.now()) });
      const urls = addon(userId, URLS, 1);
      await refresh(userId);
      await runBillingJobs(context.env, Date.now());
      urls.cancel_at_period_end = false;
      await refresh(userId);
      await runBillingJobs(context.env, Date.now());
      expect(polar.canceled.map((c) => c.id)).toEqual([urls.id, urls.id]);
    });

    it("refreshes first, so a plan back since the cancellation was recorded keeps its add-ons", async () => {
      const { userId } = await proUser();
      addon(userId, MACHINE, 1);
      Object.assign(polar.subscriptions[0]!, { status: "canceled", ended_at: iso(Date.now()) });
      await refresh(userId);
      expect(cancellations()).toHaveLength(1);

      // Pro comes back; the refresh that sees it is due in the same run.
      subscription(userId);
      run("UPDATE billing_revisions SET check_due_at = ? WHERE user_id = ?", Date.now(), userId);
      await runBillingJobs(context.env, Date.now());
      expect(polar.canceled).toEqual([]);
      expect(cancellations()).toEqual([]);
    });

    it("re-reads the snapshot before each call, and never sets an add-on to renew again", async () => {
      const { userId } = await proUser();
      const machine = addon(userId, MACHINE, 1);
      const urls = addon(userId, URLS, 1);
      Object.assign(polar.subscriptions[0]!, { status: "canceled", ended_at: iso(Date.now()) });
      await refresh(userId);
      expect(cancellations().map((c) => c.subscription_id)).toEqual([machine.id, urls.id]);

      // While Polar applies the first cancellation, Pro comes back and a
      // refresh commits it.
      const key = `PATCH ${POLAR}/v1/subscriptions/`;
      const answer = context.routes.get(key)!;
      let interleaved = false;
      context.routes.set(key, async (body, url) => {
        if (!interleaved) {
          interleaved = true;
          subscription(userId);
          await refresh(userId);
        }
        return answer(body, url);
      });
      vi.spyOn(console, "warn").mockImplementation(() => {});
      await runBillingJobs(context.env, Date.now());

      // The first cancellation stands; the second, read again before its
      // call, is never made. Nothing renews an add-on again.
      expect(patches()).toEqual([{ cancel_at_period_end: true }]);
      expect(machine.cancel_at_period_end).toBe(true);
      expect(urls.cancel_at_period_end).toBe(false);
      await refresh(userId);
      expect(cancellations()).toEqual([]);
      await runBillingJobs(context.env, Date.now() + 60 * 60 * 1000);
      expect(patches()).toEqual([{ cancel_at_period_end: true }]);
    });

    it("lets a cancellation whose answer was lost stand when Pro comes back, without looping", async () => {
      const { userId } = await proUser();
      const machine = addon(userId, MACHINE, 1);
      Object.assign(polar.subscriptions[0]!, { status: "canceled", ended_at: iso(Date.now()) });
      await refresh(userId);
      vi.spyOn(console, "error").mockImplementation(() => {});

      // Polar applies it, but the answer never arrives.
      const key = `PATCH ${POLAR}/v1/subscriptions/`;
      const answer = context.routes.get(key)!;
      context.routes.set(key, async (body, url) => {
        await answer(body, url);
        throw new TypeError("network connection lost");
      });
      const now = Date.now();
      await runBillingJobs(context.env, now);
      expect(machine.cancel_at_period_end).toBe(true);
      expect(get("SELECT state, attempts FROM addon_cancellations")).toEqual({
        state: "pending",
        attempts: 1,
      });

      // Pro comes back before the retry, and D1 knows it while the record
      // is still there: the job calls nothing, keeps the record, and queues
      // the refresh that settles it.
      context.routes.set(key, answer);
      subscription(userId);
      run(
        "UPDATE billing SET pro_status = 'active', ended_at = NULL, current_period_end = ? WHERE user_id = ?",
        now + 30 * DAY,
        userId,
      );
      await runBillingJobs(context.env, now + 5 * 60 * 1000);
      expect(patches()).toEqual([{ cancel_at_period_end: true }]);
      expect(cancellations()).toHaveLength(1);
      expect(get("SELECT check_due_at FROM billing_revisions WHERE user_id = ?", userId)).toEqual({
        check_due_at: now + 5 * 60 * 1000,
      });

      // The next run refreshes first: the subscription reads as canceling,
      // so the record goes. The add-on stays canceled.
      await runBillingJobs(context.env, now + 10 * 60 * 1000);
      expect(cancellations()).toEqual([]);
      await runBillingJobs(context.env, now + 20 * 60 * 1000);
      expect(patches()).toEqual([{ cancel_at_period_end: true }]);
      expect(machine.cancel_at_period_end).toBe(true);
    });

    it("leaves them alone when Pro comes back before the job runs", async () => {
      const { userId } = await proUser();
      addon(userId, MACHINE, 1);
      Object.assign(polar.subscriptions[0]!, { status: "canceled", ended_at: iso(Date.now()) });
      await refresh(userId);
      expect(cancellations()).toHaveLength(1);

      subscription(userId);
      await refresh(userId);
      await runBillingJobs(context.env, Date.now());
      expect(polar.canceled).toEqual([]);
      expect(cancellations()).toEqual([]);
    });

    it("retries a failed cancellation after 5 minutes, and waits out the rate limit", async () => {
      const { userId } = await proUser();
      addon(userId, MACHINE, 1);
      Object.assign(polar.subscriptions[0]!, { status: "canceled", ended_at: iso(Date.now()) });
      await refresh(userId);
      vi.spyOn(console, "error").mockImplementation(() => {});
      vi.spyOn(console, "warn").mockImplementation(() => {});

      const key = `PATCH ${POLAR}/v1/subscriptions/`;
      const answer = context.routes.get(key)!;
      context.routes.set(key, () => jsonResponse({ error: "Internal" }, 500));
      const now = Date.now();
      await runBillingJobs(context.env, now);
      expect(get("SELECT attempts, next_attempt_at FROM addon_cancellations")).toEqual({
        attempts: 1,
        next_attempt_at: now + 5 * 60 * 1000,
      });
      await runBillingJobs(context.env, now + 60 * 1000);
      expect(polarCalls("PATCH", "/v1/subscriptions/")).toHaveLength(1);

      context.routes.set(
        key,
        () =>
          new Response(JSON.stringify({ error: "RateLimited" }), {
            status: 429,
            headers: { "retry-after": "120" },
          }),
      );
      await runBillingJobs(context.env, now + 5 * 60 * 1000);
      expect(get("SELECT attempts, next_attempt_at FROM addon_cancellations")).toEqual({
        attempts: 1,
        next_attempt_at: now + 5 * 60 * 1000 + 120 * 1000,
      });

      context.routes.set(key, answer);
      await runBillingJobs(context.env, now + 8 * 60 * 1000);
      expect(polar.canceled).toHaveLength(1);
    });
  });
});
