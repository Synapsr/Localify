/// <reference types="node" />
// Billing through Polar (docs/public-urls.md, "Billing"; ADR 0024): the
// webhooks, the refresh, the lifecycle, checkouts, the portal, duplicate
// subscriptions and the reconciliation, against a fake Polar.

import { createHmac } from "node:crypto";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { proCheckout, reconcileBilling, refreshBilling, runBillingJobs } from "../src/billing";
import { PolarApi } from "../src/polar";
import {
  api,
  createContext,
  jsonResponse,
  POLAR_WEBHOOK_SECRET,
  serve,
  settle,
  signIn,
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
  /** Orders by subscription id. */
  orders: Map<string, Raw[]>;
  /** Answers `POST /v1/checkouts/`. */
  checkout: (body: Raw) => Response | Promise<Response>;
  /** Every refund created, with its current status. */
  refunds: Raw[];
  /** The status a new refund reaches at once: `pending` leaves it for `settleRefund`. */
  refundStatus: "pending" | "succeeded" | "failed" | "canceled";
  revoked: string[];
};

function orderOf(fake: FakePolar, orderId: string): Raw | undefined {
  return [...fake.orders.values()].flat().find((order) => order.id === orderId);
}

/** Moves a refund to `status`; a success takes its amount off the order. */
function settleRefund(fake: FakePolar, refundId: string, status: string): void {
  const refund = fake.refunds.find((r) => r.id === refundId)!;
  refund.status = status;
  if (status !== "succeeded") return;
  const order = orderOf(fake, String(refund.order_id));
  if (!order) return;
  order.refundable_amount = Number(order.refundable_amount) - Number(refund.amount);
  order.status = Number(order.refundable_amount) <= 0 ? "refunded" : "partially_refunded";
}

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
    orders: new Map(),
    checkout: () =>
      jsonResponse(
        {
          id: `checkout-${++checkouts}`,
          url: `${POLAR}/checkout/${checkouts}`,
          expires_at: iso(Date.now() + 60 * 60 * 1000),
        },
        201,
      ),
    refunds: [],
    refundStatus: "succeeded",
    revoked: [],
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
  context.routes.set(`GET ${POLAR}/v1/orders/`, (_body, url) => {
    const id = new URL(url).searchParams.get("subscription_id") ?? "";
    return page(fake.orders.get(id) ?? [], url);
  });
  context.routes.set(`GET ${POLAR}/v1/refunds/`, (_body, url) => {
    const id = new URL(url).searchParams.get("order_id");
    return page(
      fake.refunds.filter((refund) => refund.order_id === id),
      url,
    );
  });
  context.routes.set(`POST ${POLAR}/v1/refunds/`, (body) => {
    const request = JSON.parse(body ?? "{}") as Raw;
    const order = orderOf(fake, String(request.order_id));
    if (order && Number(order.refundable_amount) <= 0) {
      return jsonResponse({ error: "RefundedAlready", detail: "Already refunded" }, 403);
    }
    if (order && order.status !== "paid" && order.status !== "partially_refunded") {
      return jsonResponse({ error: "PaymentError", detail: "The order is not paid" }, 422);
    }
    const refund: Raw = {
      id: `refund-${fake.refunds.length + 1}`,
      ...request,
      status: "pending",
    };
    fake.refunds.push(refund);
    settleRefund(fake, String(refund.id), fake.refundStatus);
    return jsonResponse(refund, 201);
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
    const now = Date.now();
    await expect(
      refreshBilling(context.env, userId, new PolarApi(context.env), now),
    ).rejects.toThrow();
    expect(
      get("SELECT attempts, next_attempt_at FROM webhook_events WHERE id = 'msg_limited'"),
    ).toEqual({ attempts: 0, next_attempt_at: now + 120_000 });
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
    const sub = subscription(userId);
    polar.orders.set(String(sub.id), [{ id: "order-1", status: "refunded", refundable_amount: 0 }]);
    await refresh(userId);
    expect(await plan(token)).toMatchObject({ status: "active", entitled: true });
    expect(polarCalls("GET", "/v1/orders/")).toEqual([]);
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
  it("keep the one ending latest, revoke the other and refund it in full", async () => {
    const { token, userId } = await signIn(context, "teo-goulois", 1, "laptop", {
      entitled: false,
    });
    const monthly = subscription(userId, { current_period_end: iso(Date.now() + 20 * DAY) });
    const yearly = subscription(userId, {
      product_id: YEARLY,
      recurring_interval: "year",
      current_period_end: iso(Date.now() + 360 * DAY),
    });
    polar.orders.set(String(monthly.id), [
      {
        id: "order-paid",
        status: "paid",
        net_amount: 799,
        refunded_amount: 0,
        refundable_amount: 799,
      },
      {
        id: "order-trial",
        status: "paid",
        net_amount: 0,
        refunded_amount: 0,
        refundable_amount: 0,
      },
    ]);
    const alerts = vi.spyOn(console, "error").mockImplementation(() => {});

    await refresh(userId);
    expect(billingRow(userId)).toMatchObject({
      pro_subscription_id: yearly.id,
      pro_interval: "year",
    });
    expect(alerts.mock.calls.some((call) => String(call[0]).includes(String(monthly.id)))).toBe(
      true,
    );
    expect(
      get("SELECT subscription_id, revoked_at, refunded_at FROM duplicate_subscriptions"),
    ).toEqual({
      subscription_id: monthly.id,
      revoked_at: null,
      refunded_at: null,
    });
    // Recorded before any call: the refresh itself revokes nothing.
    expect(polar.revoked).toEqual([]);

    await runBillingJobs(context.env, Date.now());
    expect(polar.revoked).toEqual([monthly.id]);
    expect(polar.refunds).toMatchObject([
      { order_id: "order-paid", reason: "duplicate", amount: 799, status: "succeeded" },
    ]);
    expect(get("SELECT order_id, refund_id, amount, status FROM duplicate_refunds")).toEqual({
      order_id: "order-paid",
      refund_id: "refund-1",
      amount: 799,
      status: "succeeded",
    });
    const row = get<{ revoked_at: number | null; refunded_at: number | null }>(
      "SELECT revoked_at, refunded_at FROM duplicate_subscriptions",
    );
    expect(row.revoked_at).not.toBeNull();
    expect(row.refunded_at).not.toBeNull();

    // Revoked, it is no duplicate any more, and the kept one still serves.
    await refresh(userId);
    expect(await plan(token)).toMatchObject({ status: "active", interval: "year", entitled: true });
    await runBillingJobs(context.env, Date.now() + DAY);
    expect(polar.revoked).toHaveLength(1);
    expect(polar.refunds).toHaveLength(1);
  });

  /** A user with a duplicate, recorded by a refresh, whose orders are `orders`. */
  async function withDuplicate(orders: Raw[]) {
    const { userId } = await signIn(context, "teo-goulois", 1, "laptop", { entitled: false });
    const duplicate = subscription(userId, { current_period_end: iso(Date.now() + 10 * DAY) });
    subscription(userId, { current_period_end: iso(Date.now() + 20 * DAY) });
    polar.orders.set(String(duplicate.id), orders);
    vi.spyOn(console, "error").mockImplementation(() => {});
    vi.spyOn(console, "warn").mockImplementation(() => {});
    await refresh(userId);
    return { userId, duplicate };
  }

  type Duplicate = {
    revoked_at: number | null;
    refunded_at: number | null;
    attempts: number;
    next_attempt_at: number | null;
  };
  function duplicateRow(): Duplicate {
    return get<Duplicate>(
      "SELECT revoked_at, refunded_at, attempts, next_attempt_at FROM duplicate_subscriptions",
    );
  }
  function refundRow(orderId: string) {
    return get<{ refund_id: string | null; status: string; error: string | null }>(
      "SELECT refund_id, status, error FROM duplicate_refunds WHERE order_id = ?",
      orderId,
    );
  }

  it("retry a failed step after 5 minutes, doubling, and count a refund already made as settled", async () => {
    const { duplicate } = await withDuplicate([
      { id: "order-1", status: "paid", refundable_amount: 799 },
    ]);

    // The refund fails once: 5 minutes.
    context.routes.set(`POST ${POLAR}/v1/refunds/`, () => jsonResponse({ error: "Oops" }, 500));
    const now = Date.now();
    await runBillingJobs(context.env, now);
    const failed = duplicateRow();
    expect(failed.revoked_at).not.toBeNull();
    expect(failed.refunded_at).toBeNull();
    expect(failed).toMatchObject({ attempts: 1, next_attempt_at: now + 5 * 60 * 1000 });
    expect(refundRow("order-1")).toMatchObject({ status: "failed", error: "500 Oops" });

    // Twice: 10 minutes.
    await runBillingJobs(context.env, failed.next_attempt_at!);
    expect(duplicateRow()).toMatchObject({
      attempts: 2,
      next_attempt_at: failed.next_attempt_at! + 10 * 60 * 1000,
    });

    // Polar made it after all.
    context.routes.set(`POST ${POLAR}/v1/refunds/`, () =>
      jsonResponse({ error: "RefundedAlready", detail: "Already" }, 403),
    );
    await runBillingJobs(context.env, duplicateRow().next_attempt_at!);
    expect(duplicateRow().refunded_at).not.toBeNull();
    expect(polar.revoked).toEqual([duplicate.id]);
  });

  it("keep the obligation open while a refund is pending, until it succeeds", async () => {
    await withDuplicate([{ id: "order-1", status: "paid", refundable_amount: 799 }]);
    polar.refundStatus = "pending";
    const now = Date.now();
    await runBillingJobs(context.env, now);
    expect(polar.refunds).toMatchObject([{ id: "refund-1", status: "pending" }]);
    expect(refundRow("order-1")).toMatchObject({ refund_id: "refund-1", status: "pending" });
    // Waiting is no failure: looked at again in 5 minutes.
    expect(duplicateRow()).toMatchObject({
      refunded_at: null,
      attempts: 0,
      next_attempt_at: now + 5 * 60 * 1000,
    });

    // Still pending: nothing new.
    await runBillingJobs(context.env, now + 5 * 60 * 1000);
    expect(polar.refunds).toHaveLength(1);
    expect(duplicateRow().refunded_at).toBeNull();

    settleRefund(polar, "refund-1", "succeeded");
    await runBillingJobs(context.env, now + 10 * 60 * 1000);
    expect(polar.refunds).toHaveLength(1);
    expect(refundRow("order-1")).toMatchObject({ refund_id: "refund-1", status: "succeeded" });
    expect(duplicateRow().refunded_at).not.toBeNull();
  });

  it("retry a refund that ends failed, after the backoff", async () => {
    await withDuplicate([{ id: "order-1", status: "paid", refundable_amount: 799 }]);
    polar.refundStatus = "pending";
    const now = Date.now();
    await runBillingJobs(context.env, now);
    settleRefund(polar, "refund-1", "failed");

    // Seen failed: recorded, and retried after 5 minutes.
    const seen = now + 5 * 60 * 1000;
    await runBillingJobs(context.env, seen);
    expect(refundRow("order-1")).toMatchObject({ refund_id: "refund-1", status: "failed" });
    expect(duplicateRow()).toMatchObject({ attempts: 1, next_attempt_at: seen + 5 * 60 * 1000 });
    expect(polar.refunds).toHaveLength(1);

    polar.refundStatus = "succeeded";
    await runBillingJobs(context.env, seen + 5 * 60 * 1000);
    expect(polar.refunds).toMatchObject([
      { id: "refund-1", status: "failed" },
      { id: "refund-2", status: "succeeded", amount: 799 },
    ]);
    expect(refundRow("order-1")).toMatchObject({ refund_id: "refund-2", status: "succeeded" });
    expect(duplicateRow().refunded_at).not.toBeNull();
  });

  it("never refund twice after a crash between the refund and its record", async () => {
    await withDuplicate([
      { id: "order-1", status: "paid", refundable_amount: 799 },
      { id: "order-2", status: "paid", refundable_amount: 799 },
    ]);
    polar.refundStatus = "pending";
    // Polar makes the refund of order-1, and the answer never arrives.
    const create = context.routes.get(`POST ${POLAR}/v1/refunds/`)!;
    context.routes.set(`POST ${POLAR}/v1/refunds/`, (body, url) => {
      const answer = create(body, url);
      if (JSON.parse(body ?? "{}").order_id === "order-1") throw new Error("connection reset");
      return answer;
    });
    const now = Date.now();
    await runBillingJobs(context.env, now);
    expect(polar.refunds.map((refund) => refund.order_id)).toEqual(["order-1", "order-2"]);
    expect(refundRow("order-1")).toMatchObject({ refund_id: null, status: "failed" });

    // A worker that died after writing `creating` leaves the same trace.
    run("UPDATE duplicate_refunds SET status = 'creating' WHERE order_id = 'order-1'");
    context.routes.set(`POST ${POLAR}/v1/refunds/`, create);
    await runBillingJobs(context.env, duplicateRow().next_attempt_at!);
    // Found by looking the order's refunds up: adopted, not made again.
    expect(polar.refunds).toHaveLength(2);
    expect(refundRow("order-1")).toMatchObject({ refund_id: "refund-1", status: "pending" });

    settleRefund(polar, "refund-1", "succeeded");
    settleRefund(polar, "refund-2", "succeeded");
    await runBillingJobs(context.env, duplicateRow().next_attempt_at!);
    expect(polar.refunds).toHaveLength(2);
    expect(duplicateRow().refunded_at).not.toBeNull();
  });

  it("refund the paid orders only, past an unpaid renewal", async () => {
    await withDuplicate([
      // The newest first, as Polar lists them: a renewal not paid yet.
      { id: "order-renewal", status: "pending", net_amount: 799, refundable_amount: 799 },
      { id: "order-void", status: "void", refundable_amount: 799 },
      { id: "order-paid", status: "paid", refundable_amount: 799 },
    ]);
    await runBillingJobs(context.env, Date.now());
    expect(polar.refunds).toMatchObject([{ order_id: "order-paid", amount: 799 }]);
    expect(duplicateRow().refunded_at).not.toBeNull();
  });

  it("record an error on its order without holding back the others", async () => {
    await withDuplicate([
      { id: "order-a", status: "paid", refundable_amount: 799 },
      { id: "order-b", status: "partially_refunded", refundable_amount: 300 },
    ]);
    const create = context.routes.get(`POST ${POLAR}/v1/refunds/`)!;
    context.routes.set(`POST ${POLAR}/v1/refunds/`, (body, url) =>
      JSON.parse(body ?? "{}").order_id === "order-a"
        ? jsonResponse({ error: "NotPermitted", detail: "No" }, 400)
        : create(body, url),
    );
    await runBillingJobs(context.env, Date.now());
    expect(polar.refunds).toMatchObject([{ order_id: "order-b", amount: 300 }]);
    expect(refundRow("order-a")).toMatchObject({ status: "failed", error: "400 NotPermitted" });
    expect(refundRow("order-b")).toMatchObject({ status: "succeeded" });
    expect(duplicateRow()).toMatchObject({ refunded_at: null, attempts: 1 });
  });

  it("wait out Polar's rate limit, which is no failure", async () => {
    await withDuplicate([{ id: "order-1", status: "paid", refundable_amount: 799 }]);
    context.routes.set(
      `POST ${POLAR}/v1/refunds/`,
      () =>
        new Response(JSON.stringify({ error: "TooManyRequests" }), {
          status: 429,
          headers: { "retry-after": "30" },
        }),
    );
    const now = Date.now();
    await runBillingJobs(context.env, now);
    expect(duplicateRow()).toMatchObject({
      refunded_at: null,
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
    for (const product of ["addon_machine", "prod-monthly", ""]) {
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
    const a = proCheckout(context.env, userId, "month", null, start);
    await vi.waitFor(() => expect(held).toHaveLength(1));
    // A's call takes more than 30 seconds: B takes the row over.
    const b = proCheckout(context.env, userId, "year", null, start + 31_000);
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
    const a = proCheckout(context.env, userId, "month", null, start);
    await vi.waitFor(() => expect(held).toHaveLength(1));
    const b = proCheckout(context.env, userId, "month", null, start + 31_000);
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
