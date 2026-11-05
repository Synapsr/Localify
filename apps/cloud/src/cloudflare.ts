// The Cloudflare API calls behind machine registration: one named tunnel per
// machine, configured by Cloudflare, and a proxied CNAME to it.

import { setting } from "./config";

const API = "https://api.cloudflare.com/client/v4";

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

  constructor(env: Env) {
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

  async deleteDnsRecord(recordId: string): Promise<void> {
    await this.callAllowingMissing("DELETE", `${API}/zones/${this.zoneId}/dns_records/${recordId}`);
  }

  /** Drops the connections `cloudflared` holds, so the tunnel can be deleted. */
  async cleanConnections(tunnelId: string): Promise<void> {
    await this.callAllowingMissing("DELETE", `${this.tunnels()}/${tunnelId}/connections`);
  }

  async deleteTunnel(tunnelId: string): Promise<void> {
    await this.callAllowingMissing("DELETE", `${this.tunnels()}/${tunnelId}`);
  }

  /** The id of a live tunnel with this name, for a sign-in that stopped before recording it. */
  async findTunnelId(name: string): Promise<string | null> {
    const query = new URLSearchParams({ name, is_deleted: "false" });
    const result = await this.call<{ id?: unknown }[]>("GET", `${this.tunnels()}?${query}`);
    const id = result?.[0]?.id;
    return typeof id === "string" ? id : null;
  }

  /** The id of the CNAME for this hostname, for a sign-in that stopped before recording it. */
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

  /** A delete of something already gone succeeds. */
  private async callAllowingMissing(method: string, url: string): Promise<void> {
    try {
      await this.call(method, url);
    } catch (error) {
      if (error instanceof CloudflareError && error.status === 404) return;
      throw error;
    }
  }

  private async call<T>(method: string, url: string, body?: unknown): Promise<T | undefined> {
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
