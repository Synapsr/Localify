// The Polar API calls behind the paid plan (docs/public-urls.md, "Billing";
// ADR 0024): checkouts, customer portal sessions, the subscription list, and
// the revocation of a duplicate subscription. Never called on the public
// path. Refunds are an operator's, in Polar's dashboard.

import type { CallBudget } from "./cloudflare";

import { setting } from "./config";

/** Every call pins this version, as the webhook endpoint does. */
export const POLAR_VERSION = "2026-10";
/** Results per page of Polar's listings, its maximum. */
const PAGE_SIZE = 100;

/** How long to wait after a 429 that says nothing. */
const DEFAULT_RETRY_AFTER_MS = 60 * 1000;

/** A failed Polar API call. `code` is Polar's error name. The message never contains the token. */
export class PolarError extends Error {
  constructor(
    readonly status: number,
    readonly code: string | null,
    message: string,
    /** For a 429, when to try again. */
    readonly retryAfterMs: number | null = null,
  ) {
    super(message);
  }
}

/**
 * Polar refused the call for its rate limit: a reason to wait, not a
 * failure.
 */
export function isRateLimited(error: unknown): error is PolarError & { retryAfterMs: number } {
  return error instanceof PolarError && error.status === 429;
}

/** A subscription as the list gives it. Times are Unix milliseconds. */
export type PolarSubscription = {
  id: string;
  status: string;
  productId: string;
  customerId: string | null;
  recurringInterval: string | null;
  cancelAtPeriodEnd: boolean;
  currentPeriodEnd: number | null;
  trialStart: number | null;
  trialEnd: number | null;
  startedAt: number | null;
  endsAt: number | null;
  endedAt: number | null;
  pastDueAt: number | null;
  units: number | null;
  externalCustomerId: string | null;
};

/** One page of a listing, and how many pages there are. */
export type Page<T> = { items: T[]; maxPage: number };

export type CheckoutRequest = {
  products: string[];
  externalCustomerId: string;
  allowTrial: boolean;
  userId: string;
  successUrl: string;
  returnUrl: string;
  /** The buyer's address: Polar picks the currency from it. */
  customerIpAddress: string | null;
};

export type Checkout = { id: string; url: string; expiresAt: number };

export class PolarApi {
  private readonly base: string;
  private readonly token: string;

  constructor(
    env: Env,
    private readonly budget: CallBudget | null = null,
  ) {
    this.base = setting(env, "POLAR_API_BASE").replace(/\/+$/, "");
    this.token = setting(env, "POLAR_ACCESS_TOKEN");
  }

  /** One page, from 1, of the subscriptions of a customer, or of the organization when null. Every status. */
  async listSubscriptions(
    externalCustomerId: string | null,
    page: number,
  ): Promise<Page<PolarSubscription>> {
    const query = new URLSearchParams({ limit: String(PAGE_SIZE), page: String(page) });
    if (externalCustomerId !== null) query.set("external_customer_id", externalCustomerId);
    const result = await this.call("GET", `/v1/subscriptions/?${query}`);
    return listing(result, "subscription", toSubscription);
  }

  async createCheckout(request: CheckoutRequest): Promise<Checkout> {
    const result = (await this.call("POST", "/v1/checkouts/", {
      products: request.products,
      external_customer_id: request.externalCustomerId,
      allow_trial: request.allowTrial,
      metadata: { userId: request.userId },
      success_url: request.successUrl,
      return_url: request.returnUrl,
      ...(request.customerIpAddress === null
        ? {}
        : { customer_ip_address: request.customerIpAddress }),
    })) as { id?: unknown; url?: unknown; expires_at?: unknown } | null;
    const expiresAt = timeOf(result?.expires_at);
    if (typeof result?.id !== "string" || typeof result.url !== "string" || expiresAt === null) {
      throw new PolarError(502, null, "The checkout Polar created has no id, url or expiry");
    }
    return { id: result.id, url: result.url, expiresAt };
  }

  /** The customer portal URL of a session for this customer. */
  async createCustomerSession(externalCustomerId: string, returnUrl: string): Promise<string> {
    const result = (await this.call("POST", "/v1/customer-sessions/", {
      external_customer_id: externalCustomerId,
      return_url: returnUrl,
    })) as { customer_portal_url?: unknown } | null;
    if (typeof result?.customer_portal_url !== "string") {
      throw new PolarError(502, null, "The customer session Polar created has no portal URL");
    }
    return result.customer_portal_url;
  }

  /** Ends a subscription now. One already revoked counts as done. */
  async revokeSubscription(id: string): Promise<void> {
    try {
      await this.call("DELETE", `/v1/subscriptions/${encodeURIComponent(id)}`);
    } catch (error) {
      if (error instanceof PolarError && error.code === "AlreadyCanceledSubscription") return;
      throw error;
    }
  }

  private async call(method: string, path: string, body?: unknown): Promise<unknown> {
    this.budget?.spend();
    let response: Response;
    try {
      response = await fetch(`${this.base}${path}`, {
        method,
        headers: {
          authorization: `Bearer ${this.token}`,
          accept: "application/json",
          "polar-version": POLAR_VERSION,
          ...(body === undefined ? {} : { "content-type": "application/json" }),
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (error) {
      throw new PolarError(502, null, `${method} ${path} failed: ${String(error)}`);
    }
    const text = await response.text().catch(() => "");
    let parsed: unknown = null;
    try {
      parsed = text === "" ? null : JSON.parse(text);
    } catch {
      parsed = null;
    }
    if (!response.ok) {
      const code =
        typeof (parsed as { error?: unknown } | null)?.error === "string"
          ? (parsed as { error: string }).error
          : null;
      const retryAfter = Number(response.headers.get("retry-after"));
      throw new PolarError(
        response.status,
        code,
        `${method} ${path} answered ${response.status}${code ? ` ${code}` : ""}`,
        response.status === 429
          ? Number.isFinite(retryAfter) && retryAfter > 0
            ? retryAfter * 1000
            : DEFAULT_RETRY_AFTER_MS
          : null,
      );
    }
    return parsed;
  }
}

function listing<T>(
  result: unknown,
  what: string,
  read: (item: Record<string, unknown>) => T | null,
): Page<T> {
  const { items, pagination } = (result ?? {}) as { items?: unknown; pagination?: unknown };
  const maxPage = (pagination as { max_page?: unknown } | undefined)?.max_page;
  if (!Array.isArray(items) || typeof maxPage !== "number") {
    throw new PolarError(502, null, `The ${what} listing has no items or pagination`);
  }
  return {
    items: items.flatMap((item) => {
      const value = typeof item === "object" && item !== null ? read(item) : null;
      return value === null ? [] : [value];
    }),
    maxPage,
  };
}

function toSubscription(item: Record<string, unknown>): PolarSubscription | null {
  if (
    typeof item.id !== "string" ||
    typeof item.status !== "string" ||
    typeof item.product_id !== "string"
  )
    return null;
  const customer = item.customer as { external_id?: unknown } | null | undefined;
  return {
    id: item.id,
    status: item.status,
    productId: item.product_id,
    customerId: typeof item.customer_id === "string" ? item.customer_id : null,
    recurringInterval: typeof item.recurring_interval === "string" ? item.recurring_interval : null,
    cancelAtPeriodEnd: item.cancel_at_period_end === true,
    currentPeriodEnd: timeOf(item.current_period_end),
    trialStart: timeOf(item.trial_start),
    trialEnd: timeOf(item.trial_end),
    startedAt: timeOf(item.started_at),
    endsAt: timeOf(item.ends_at),
    endedAt: timeOf(item.ended_at),
    pastDueAt: timeOf(item.past_due_at),
    units: typeof item.units === "number" ? item.units : null,
    externalCustomerId: typeof customer?.external_id === "string" ? customer.external_id : null,
  };
}

/** An RFC 3339 time in Unix milliseconds; null when missing or unreadable. */
function timeOf(value: unknown): number | null {
  if (typeof value !== "string") return null;
  const ms = Date.parse(value);
  return Number.isFinite(ms) ? ms : null;
}
