// The Polar API calls behind the paid plan (docs/public-urls.md, "Billing";
// ADR 0024): checkouts, customer portal sessions, the subscription list, the
// revocation of a duplicate subscription, the cancellation of the add-ons of
// a lapsed plan, and the products behind `/v1/pricing`. Never called on the
// public path. Refunds are an operator's, in Polar's dashboard.

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
  /** For a unit-priced product, the quantity the page starts with. */
  units: number | null;
};

export type Checkout = { id: string; url: string; expiresAt: number };

/**
 * One price of a product in one currency, in minor units: a fixed price's
 * amount, or a unit-based price's rate for its first tier.
 */
export type PolarPrice = { currency: string; amount: number };

export type PolarProduct = {
  id: string;
  trialInterval: string | null;
  trialIntervalCount: number | null;
  /** The prices that are not archived, one per currency. */
  prices: PolarPrice[];
};

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
      ...(request.units === null ? {} : { units: request.units }),
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

  /**
   * Sets a subscription to end when its period does, without a refund. One
   * already canceling, or no longer active, counts as done.
   */
  async cancelAtPeriodEnd(id: string): Promise<void> {
    try {
      await this.call("PATCH", `/v1/subscriptions/${encodeURIComponent(id)}`, {
        cancel_at_period_end: true,
      });
    } catch (error) {
      if (
        error instanceof PolarError &&
        (error.code === "AlreadyCanceledSubscription" || error.code === "InactiveSubscription")
      )
        return;
      throw error;
    }
  }

  /** The products with these ids, in one call. Needs `products:read`. */
  async listProducts(ids: string[]): Promise<PolarProduct[]> {
    const query = new URLSearchParams({ limit: String(PAGE_SIZE) });
    for (const id of ids) query.append("id", id);
    const result = await this.call("GET", `/v1/products/?${query}`);
    return listing(result, "product", toProduct).items;
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

function toProduct(item: Record<string, unknown>): PolarProduct | null {
  if (typeof item.id !== "string" || !Array.isArray(item.prices)) return null;
  return {
    id: item.id,
    trialInterval: typeof item.trial_interval === "string" ? item.trial_interval : null,
    trialIntervalCount:
      typeof item.trial_interval_count === "number" ? item.trial_interval_count : null,
    prices: item.prices.flatMap((price: unknown) => {
      const read = typeof price === "object" && price !== null ? toPrice(price as Raw) : null;
      return read === null ? [] : [read];
    }),
  };
}

type Raw = Record<string, unknown>;

/**
 * A fixed price's `price_amount`, or a unit-based price's first tier: its
 * `unit_amount` is a decimal string of cents, rounded here. Other kinds of
 * price, and archived ones, are left out.
 */
function toPrice(price: Raw): PolarPrice | null {
  if (price.is_archived === true || typeof price.price_currency !== "string") return null;
  let amount: number | null = null;
  if (price.amount_type === "fixed" && typeof price.price_amount === "number") {
    amount = price.price_amount;
  } else if (price.amount_type === "unit_based") {
    const tiers = (price.tiers as { tiers?: unknown } | null | undefined)?.tiers;
    const first = Array.isArray(tiers) ? (tiers[0] as { unit_amount?: unknown } | undefined) : null;
    const rate = Number(first?.unit_amount);
    if (typeof first?.unit_amount === "string" && Number.isFinite(rate)) amount = Math.round(rate);
  }
  if (amount === null || amount < 0) return null;
  return { currency: price.price_currency.toUpperCase(), amount };
}

/** An RFC 3339 time in Unix milliseconds; null when missing or unreadable. */
function timeOf(value: unknown): number | null {
  if (typeof value !== "string") return null;
  const ms = Date.parse(value);
  return Number.isFinite(ms) ? ms : null;
}
