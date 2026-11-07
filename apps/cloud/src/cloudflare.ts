// The Cloudflare API calls behind machines' tunnels: named tunnels configured
// by Cloudflare, and one proxied CNAME per machine that points at its
// current tunnel.

import { setting } from "./config";
import { randomBytes } from "./crypto";

const API = "https://api.cloudflare.com/client/v4";
/** Results per page when listing tunnels and DNS records. */
export const LIST_PAGE_SIZE = 1000;
/** Calls `removeTunnel` makes: a new secret, the connections, the tunnel. */
export const REMOVE_TUNNEL_CALLS = 3;

/**
 * Cloudflare API calls a scheduled run may still make. The account allows
 * 1,200 calls per 5 minutes; a run stays well below. Every request of a
 * `CloudflareApi` given a budget spends one call, and none is sent once it
 * is spent.
 */
export class CallBudget {
  constructor(public left: number) {}

  /** Whether work that makes up to `calls` requests can start. */
  has(calls: number): boolean {
    return this.left >= calls;
  }

  spend(): void {
    if (this.left < 1) throw new BudgetExhausted();
    this.left -= 1;
  }
}

/** A request refused because the run's budget is spent. Not a CloudflareError: nothing retries it. */
class BudgetExhausted extends Error {
  constructor() {
    super("The run's Cloudflare call budget is spent");
  }
}

/** A tunnel as the account's listing gives it. `status` is Cloudflare's. */
export type ListedTunnel = { id: string; name: string; status: string; createdAt: number };

/** A CNAME of the machine zone. */
export type ListedRecord = { id: string; name: string; content: string };

/** A failed Cloudflare API call. The message never contains the API token. */
export class CloudflareError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

type Envelope<T> = {
  success?: boolean;
  errors?: { code?: number; message?: string }[];
  result?: T;
};

export class CloudflareApi {
  private readonly accountId: string;
  private readonly zoneId: string;
  private readonly token: string;

  constructor(
    env: Env,
    private readonly budget: CallBudget | null = null,
  ) {
    this.accountId = setting(env, "CLOUDFLARE_ACCOUNT_ID");
    this.zoneId = setting(env, "MACHINE_ZONE_ID");
    this.token = setting(env, "CLOUDFLARE_API_TOKEN");
  }

  /** Creates a tunnel whose configuration Cloudflare holds; returns its id. */
  async createTunnel(name: string): Promise<string> {
    const result = await this.call<{ id?: unknown }>("POST", this.tunnels(), {
      name,
      config_src: "cloudflare",
    });
    if (typeof result?.id !== "string")
      throw new CloudflareError(502, "Tunnel creation returned no id");
    return result.id;
  }

  /** Routes `hostname` to the daemon's public ingress, and anything else to a 404. */
  async configureTunnel(tunnelId: string, hostname: string, ingressPort: number): Promise<void> {
    await this.call("PUT", `${this.tunnels()}/${tunnelId}/configurations`, {
      config: {
        ingress: [
          { hostname, service: `http://127.0.0.1:${ingressPort}` },
          { service: "http_status:404" },
        ],
      },
    });
  }

  /** The token `cloudflared tunnel run` needs. */
  async tunnelToken(tunnelId: string): Promise<string> {
    const result = await this.call<unknown>("GET", `${this.tunnels()}/${tunnelId}/token`);
    if (typeof result !== "string" || result === "") {
      throw new CloudflareError(502, "Tunnel token request returned no token");
    }
    return result;
  }

  /** The proxied CNAME `hostname → <tunnel id>.cfargotunnel.com`; returns the record id. */
  async createTunnelRecord(hostname: string, tunnelId: string, comment: string): Promise<string> {
    const result = await this.call<{ id?: unknown }>(
      "POST",
      `${API}/zones/${this.zoneId}/dns_records`,
      {
        type: "CNAME",
        name: hostname,
        content: `${tunnelId}.cfargotunnel.com`,
        proxied: true,
        ttl: 1,
        comment,
      },
    );
    if (typeof result?.id !== "string")
      throw new CloudflareError(502, "DNS record creation returned no id");
    return result.id;
  }

  /** Moves an existing CNAME to `<tunnel id>.cfargotunnel.com`. */
  async pointRecord(recordId: string, tunnelId: string): Promise<void> {
    await this.call("PATCH", `${API}/zones/${this.zoneId}/dns_records/${recordId}`, {
      content: `${tunnelId}.cfargotunnel.com`,
      proxied: true,
    });
  }

  /** One page of the zone's CNAMEs, starting at 1. */
  async listRecords(page: number): Promise<ListedRecord[]> {
    const query = new URLSearchParams({
      type: "CNAME",
      per_page: String(LIST_PAGE_SIZE),
      page: String(page),
    });
    const result = await this.call<{ id?: unknown; name?: unknown; content?: unknown }[]>(
      "GET",
      `${API}/zones/${this.zoneId}/dns_records?${query}`,
    );
    if (!Array.isArray(result)) throw new CloudflareError(502, "DNS listing returned no list");
    return result.flatMap((record) =>
      typeof record.id === "string" &&
      typeof record.name === "string" &&
      typeof record.content === "string"
        ? [{ id: record.id, name: record.name, content: record.content }]
        : [],
    );
  }

  /** One page of the account's live tunnels, starting at 1. */
  async listTunnels(page: number): Promise<ListedTunnel[]> {
    const query = new URLSearchParams({
      is_deleted: "false",
      per_page: String(LIST_PAGE_SIZE),
      page: String(page),
    });
    const result = await this.call<
      { id?: unknown; name?: unknown; status?: unknown; created_at?: unknown }[]
    >("GET", `${this.tunnels()}?${query}`);
    if (!Array.isArray(result)) throw new CloudflareError(502, "Tunnel listing returned no list");
    return result.flatMap((tunnel) => {
      const createdAt = typeof tunnel.created_at === "string" ? Date.parse(tunnel.created_at) : NaN;
      return typeof tunnel.id === "string" &&
        typeof tunnel.name === "string" &&
        typeof tunnel.status === "string" &&
        Number.isFinite(createdAt)
        ? [{ id: tunnel.id, name: tunnel.name, status: tunnel.status, createdAt }]
        : [];
    });
  }

  async deleteDnsRecord(recordId: string): Promise<void> {
    await this.callAllowingMissing("DELETE", `${API}/zones/${this.zoneId}/dns_records/${recordId}`);
  }

  /**
   * Removes a tunnel so that nothing can serve through it again. A new
   * secret first: dropping the connections alone leaves the token valid, and
   * `cloudflared` would reconnect before the delete. Then the connections,
   * which closes every open stream, then the tunnel. What is already gone
   * counts as done.
   */
  async removeTunnel(tunnelId: string): Promise<void> {
    await this.callAllowingMissing("PATCH", `${this.tunnels()}/${tunnelId}`, {
      tunnel_secret: btoa(String.fromCharCode(...randomBytes(32))),
    });
    await this.callAllowingMissing("DELETE", `${this.tunnels()}/${tunnelId}/connections`);
    await this.callAllowingMissing("DELETE", `${this.tunnels()}/${tunnelId}`);
  }

  /** The id of a live tunnel with this name, for an operation that stopped before recording it. */
  async findTunnelId(name: string): Promise<string | null> {
    const query = new URLSearchParams({ name, is_deleted: "false" });
    const result = await this.call<{ id?: unknown }[]>("GET", `${this.tunnels()}?${query}`);
    const id = result?.[0]?.id;
    return typeof id === "string" ? id : null;
  }

  /** The id of the CNAME for this hostname, for an operation that stopped before recording it. */
  async findDnsRecordId(hostname: string): Promise<string | null> {
    const query = new URLSearchParams({ type: "CNAME", name: hostname });
    const result = await this.call<{ id?: unknown }[]>(
      "GET",
      `${API}/zones/${this.zoneId}/dns_records?${query}`,
    );
    const id = result?.[0]?.id;
    return typeof id === "string" ? id : null;
  }

  private tunnels(): string {
    return `${API}/accounts/${this.accountId}/cfd_tunnel`;
  }

  /** A change to something already gone succeeds. */
  private async callAllowingMissing(method: string, url: string, body?: unknown): Promise<void> {
    try {
      await this.call(method, url, body);
    } catch (error) {
      if (error instanceof CloudflareError && error.status === 404) return;
      throw error;
    }
  }

  private async call<T>(method: string, url: string, body?: unknown): Promise<T | undefined> {
    this.budget?.spend();
    let response: Response;
    try {
      response = await fetch(url, {
        method,
        headers: {
          authorization: `Bearer ${this.token}`,
          ...(body === undefined ? {} : { "content-type": "application/json" }),
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (error) {
      throw new CloudflareError(502, `${method} ${url} failed: ${String(error)}`);
    }
    const envelope = (await response.json().catch(() => null)) as Envelope<T> | null;
    if (!response.ok || envelope?.success !== true) {
      const reasons = (envelope?.errors ?? [])
        .map((error) => `${error.code ?? "?"} ${error.message ?? ""}`.trim())
        .join("; ");
      throw new CloudflareError(
        response.status,
        `${method} ${url} answered ${response.status}: ${reasons}`,
      );
    }
    return envelope.result;
  }
}
