/// <reference types="node" />
// The sandbox environment of wrangler.jsonc (apps/cloud/README.md,
// "Sandbox"): its own Worker, D1 and Polar, never production's routes, and
// no Cloudflare call, so it cannot touch production's tunnels or records.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import ts from "typescript";
import { afterEach, describe, expect, it, vi } from "vitest";

import { runBillingJobs } from "../src/billing";
import { runSweep } from "../src/tunnels";
import { api, createContext, serve, signIn } from "./support";

type Config = {
  name: string;
  workers_dev?: boolean;
  routes?: unknown[];
  vars: Record<string, string | number>;
  d1_databases: { binding: string; database_name: string; database_id: string }[];
  ratelimits: { name: string; namespace_id: string; simple: unknown }[];
  durable_objects: { bindings: { name: string; class_name: string }[] };
  secrets: { required: string[] };
  env: Record<string, Config>;
};

const WRANGLER = join(import.meta.dirname, "..", "wrangler.jsonc");
const { config: top } = ts.parseConfigFileTextToJson(WRANGLER, readFileSync(WRANGLER, "utf8")) as {
  config: Config;
};
const sandbox = top.env.sandbox!;

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("the sandbox environment", () => {
  it("is its own Worker on workers.dev, with no route or custom domain", () => {
    expect(sandbox.name).toBe("localify-cloud-sandbox");
    expect(sandbox.workers_dev).toBe(true);
    // Routes are inherited: an absent key would take production's.
    expect(sandbox.routes).toEqual([]);
  });

  it("has its own D1 database", () => {
    expect(sandbox.d1_databases).toEqual([
      expect.objectContaining({ binding: "DB", database_name: "localify-cloud-sandbox" }),
    ]);
    expect(sandbox.d1_databases[0]!.database_id).not.toBe(top.d1_databases[0]!.database_id);
  });

  it("repeats every binding and var, which environments do not inherit", () => {
    expect(Object.keys(sandbox.vars).sort()).toEqual(Object.keys(top.vars).sort());
    expect(sandbox.ratelimits.map((r) => [r.name, r.simple])).toEqual(
      top.ratelimits.map((r) => [r.name, r.simple]),
    );
    // Namespaces of its own: its traffic never counts against production's.
    const production = new Set(top.ratelimits.map((r) => r.namespace_id));
    for (const { namespace_id } of sandbox.ratelimits)
      expect(production.has(namespace_id)).toBe(false);
    expect(sandbox.durable_objects).toEqual(top.durable_objects);
  });

  it("bills through Polar's sandbox, with add-ons on sale and the founder alone signing in", () => {
    expect(sandbox.vars).toMatchObject({
      POLAR_API_BASE: "https://sandbox-api.polar.sh",
      POLAR_PRODUCT_PRO_MONTHLY: "aaf5080e-3a2c-44d4-ad49-442107637d67",
      POLAR_PRODUCT_PRO_YEARLY: "abe09656-c2ad-4942-b779-ed7ca4cc9b6e",
      POLAR_PRODUCT_ADDON_MACHINE: "65ac9781-c8c3-4573-b9d2-0e8b6923a695",
      POLAR_PRODUCT_ADDON_URLS: "6dd4a1de-e3be-42dd-96a7-fb661d33fe00",
      ADDONS: "on",
      SIGNUP: "allowlist",
      ALLOWED_GITHUB_IDS: "77238381",
      TEAM_GITHUB_IDS: "",
    });
    for (const name of Object.keys(sandbox.vars).filter((key) => key.startsWith("POLAR_PRODUCT"))) {
      expect(sandbox.vars[name], name).not.toBe(top.vars[name]);
    }
  });

  it("keeps production's add-ons off, with production's products", () => {
    expect(top.vars).toMatchObject({
      POLAR_API_BASE: "https://api.polar.sh",
      POLAR_PRODUCT_ADDON_MACHINE: "55653a9a-228d-4f23-8c66-266ec950ae7c",
      POLAR_PRODUCT_ADDON_URLS: "6385e584-ea00-4ac6-a838-7a5e0f3346c4",
      ADDONS: "off",
    });
  });

  it("never serves, on names that cannot collide with production's, without a Cloudflare token", () => {
    expect(sandbox.vars.MAX_TUNNELS).toBe(0);
    for (const name of ["PUBLIC_BASE_DOMAIN", "MACHINE_DOMAIN"]) {
      expect(String(sandbox.vars[name]).endsWith(".invalid"), name).toBe(true);
    }
    expect(sandbox.vars.MACHINE_ZONE_ID).toBe("");
    expect(sandbox.vars.CLOUDFLARE_ACCOUNT_ID).toBe("");
    expect(sandbox.secrets.required).not.toContain("CLOUDFLARE_API_TOKEN");
    expect(top.secrets.required).toContain("CLOUDFLARE_API_TOKEN");
  });

  it("runs with its vars without a single Cloudflare call, even if it held a token", async () => {
    // MAX_TUNNELS at 0 is enough on its own: a token, an account and a zone
    // here change nothing.
    const context = createContext({
      ...(sandbox.vars as unknown as Partial<Env>),
      ALLOWED_GITHUB_IDS: "1",
      CLOUDFLARE_ACCOUNT_ID: "account-1",
      MACHINE_ZONE_ID: "zone-1",
    });
    const { token } = await signIn(context);
    const created = await api(context, "POST", "/v1/public-urls", {
      token,
      body: { localDomain: "journal.localify", protection: { mode: "public" } },
    });
    expect(created.status).toBe(201);
    const runtime = await serve(context, token);
    expect(runtime.body).toMatchObject({ state: "blocked", blocked: { reason: "capacity" } });

    context.routes.set("GET https://sandbox-api.polar.sh/", () => new Response("{}"));
    await runBillingJobs(context.env, Date.now());
    await runSweep(context.env, Date.now());
    expect(context.calls.filter((call) => call.url.includes("cloudflare.com"))).toEqual([]);
  });
});
