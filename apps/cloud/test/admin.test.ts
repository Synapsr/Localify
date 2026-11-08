// `pnpm cloud:admin`, against the D1 stand-in through the same literal SQL
// it hands wrangler, with the Worker reading the result.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AdminError, bindParams, runCommand, UsageError, type AdminDb } from "../scripts/admin";
import { runSweep } from "../src/tunnels";
import { api, createContext, serve, settle, signIn, visit, type TestContext } from "./support";

let context: TestContext;

beforeEach(() => {
  context = createContext();
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/**
 * Each statement bound into SQL text, as `wrangler d1 execute --command`
 * receives it, and the call applied all or nothing, as D1 runs it. `fail`
 * makes the statements it matches throw.
 */
function adminDb(fail?: RegExp, beforeWrite?: () => void): AdminDb {
  return async (statements) => {
    // Another request lands between the command's reads and its write.
    if (beforeWrite && statements.some((statement) => /^\s*UPDATE/.test(statement.sql))) {
      beforeWrite();
      beforeWrite = undefined;
    }
    context.sqlite.exec("BEGIN");
    try {
      const results = statements.map((statement) => {
        const sql = bindParams(statement.sql, statement.params);
        if (fail?.test(sql)) throw new Error("D1 failed");
        return context.sqlite
          .prepare(sql)
          .all()
          .map((row) => ({ ...row }));
      });
      context.sqlite.exec("COMMIT");
      return results;
    } catch (error) {
      context.sqlite.exec("ROLLBACK");
      throw error;
    }
  };
}

function admin(...args: string[]): Promise<string[]> {
  return runCommand(adminDb(), args);
}

function get<T>(sql: string, ...params: (string | number)[]): T {
  return context.sqlite.prepare(sql).get(...params) as T;
}

let slugCount = 0;
async function createUrl(
  token: string,
  localDomain: string,
): Promise<{ id: string; slug: string }> {
  const slug = `admin-${Date.now().toString(36)}-${slugCount++}`;
  const response = await api(context, "POST", "/v1/public-urls", {
    token,
    body: { localDomain, slug, protection: { mode: "public" } },
  });
  expect(response.status).toBe(201);
  const url = ((await response.json()) as { publicUrl: { id: string; slug: string } }).publicUrl;
  // Two creates can share a millisecond: give each its own, in order.
  context.sqlite
    .prepare("UPDATE public_urls SET created_at = ? WHERE id = ?")
    .run(slugCount, url.id);
  return url;
}

function machine(id: string) {
  return get<{
    tunnel_state: string;
    tunnel_generation: number;
    rotate_requested_at: number | null;
  }>("SELECT tunnel_state, tunnel_generation, rotate_requested_at FROM machines WHERE id = ?", id);
}

describe("parameters", () => {
  it("become SQL literals, quotes doubled", () => {
    expect(bindParams("SELECT ?1, ?2, ?3, ?1", ["it's", 5, null])).toBe(
      "SELECT 'it''s', 5, NULL, 'it''s'",
    );
  });

  it("refuse what they cannot write safely", () => {
    expect(() => bindParams("SELECT ?2", ["a"])).toThrow(/No value for \?2/);
    expect(() => bindParams("SELECT ?1", [1.5])).toThrow(/integer/);
    expect(() => bindParams("SELECT ?1", ["a\0b"])).toThrow(/NUL/);
  });

  it("keep a value from ever becoming SQL", async () => {
    await signIn(context);
    const reason = `x'); DROP TABLE users; -- ?1 "`;
    await admin("ban", "teo-goulois", reason);
    expect(get("SELECT banned_reason FROM users")).toEqual({ banned_reason: reason });
  });
});

describe("ban and unban", () => {
  it("refuse the API, take the URLs offline, mark the tunnels, and the sweep releases them", async () => {
    const { token, machineId } = await signIn(context);
    const url = await createUrl(token, "journal.localify");
    await serve(context, token);

    const lines = await admin("ban", "Teo-Goulois", "phishing", "pages");
    expect(lines).toContain("Banned: phishing pages.");
    expect(lines).toContain("Entitlement: 5 serving machines, 10 URLs -> none.");
    expect(lines.some((line) => line.startsWith("Rotating laptop"))).toBe(true);
    expect(machine(machineId).rotate_requested_at).not.toBeNull();
    expect((await api(context, "GET", "/v1/me", { token })).status).toBe(403);
    expect((await visit(context, `https://${url.slug}.localify.fyi/`)).status).toBe(410);

    await runSweep(context.env);
    expect(machine(machineId)).toMatchObject({ tunnel_state: "none", rotate_requested_at: null });

    expect(await admin("ban", "teo-goulois", "again")).toEqual([
      expect.stringContaining("was already banned"),
      "Nothing changed.",
    ]);
    const unbanned = await admin("unban", "teo-goulois");
    expect(unbanned).toContain("Entitlement: none -> 5 serving machines, 10 URLs.");
    expect(get("SELECT banned_at, banned_reason FROM users")).toEqual({
      banned_at: null,
      banned_reason: null,
    });
    expect((await serve(context, token)).body.state).toBe("provisioning");
  });

  it("find an account by GitHub id when two last had the same login", async () => {
    await signIn(context, "teo-goulois", 1, "laptop");
    context.sqlite.exec(
      "INSERT INTO users (id, github_id, login, created_at) VALUES ('other', 2, 'teo-goulois', 2)",
    );
    await expect(admin("ban", "teo-goulois", "spam")).rejects.toThrow(
      /id:2, id:1\. Pass the one you mean/,
    );
    await admin("ban", "id:2", "spam");
    expect(
      context.sqlite.prepare("SELECT github_id FROM users WHERE banned_at IS NOT NULL").all(),
    ).toEqual([{ github_id: 2 }]);
  });
});

describe("takedown and restore", () => {
  it("take one URL down and rotate its machine, which keeps serving the others", async () => {
    const { token, machineId } = await signIn(context);
    const bad = await createUrl(token, "bad.localify");
    const good = await createUrl(token, "good.localify");
    await serve(context, token);

    const lines = await admin("takedown", bad.slug, "malware");
    expect(lines[0]).toContain("taken down: malware.");
    expect(get("SELECT taken_down_reason FROM public_urls WHERE id = ?", bad.id)).toEqual({
      taken_down_reason: "malware",
    });
    expect((await visit(context, `https://${bad.slug}.localify.fyi/`)).status).toBe(410);

    await runSweep(context.env);
    expect(machine(machineId)).toMatchObject({ tunnel_state: "none", tunnel_generation: 1 });
    // The machine still serves the other URL: its next fetch provisions generation 2.
    const next = await serve(context, token, 1);
    expect(next.body.state).toBe("provisioning");
    expect(machine(machineId)).toMatchObject({ tunnel_state: "ready", tunnel_generation: 2 });
    expect(
      (next.body.publicUrls as { id: string; state: string }[]).map((url) => [url.id, url.state]),
    ).toEqual([
      [bad.id, "taken_down"],
      [good.id, "serving"],
    ]);

    expect(await admin("takedown", bad.slug, "again")).toEqual([
      expect.stringContaining("was already taken down"),
      "Nothing changed.",
    ]);
    const restored = await admin("restore", bad.slug.toUpperCase());
    expect(restored[0]).toContain("restored; the takedown of");
    expect(get("SELECT taken_down_at FROM public_urls WHERE id = ?", bad.id)).toEqual({
      taken_down_at: null,
    });
    expect(await admin("restore", bad.slug)).toEqual([
      expect.stringContaining("is not taken down"),
      "Nothing changed.",
    ]);
  });

  it("rotate the machine of the URL a restored older one pushes beyond the limit", async () => {
    const laptop = await signIn(context, "teo-goulois", 1, "laptop");
    const runner = await signIn(context, "teo-goulois", 1, "runner");
    const old = await createUrl(laptop.token, "old.localify");
    await createUrl(runner.token, "new.localify");
    await admin("override", "teo-goulois", "5", "1", "team");
    await admin("takedown", old.slug, "spam");
    await serve(context, runner.token);
    expect(machine(runner.machineId).tunnel_state).toBe("ready");

    const lines = await admin("restore", old.slug);
    expect(lines.some((line) => line.startsWith("Rotating runner"))).toBe(true);
    expect(machine(runner.machineId).rotate_requested_at).not.toBeNull();
  });
});

describe("overrides", () => {
  it("set and change the limits, rotating every serving machine of the owner when a URL stops", async () => {
    const laptop = await signIn(context, "teo-goulois", 1, "laptop");
    const runner = await signIn(context, "teo-goulois", 1, "runner");
    await createUrl(laptop.token, "a.localify");
    await createUrl(runner.token, "b.localify");
    await serve(context, laptop.token);
    await serve(context, runner.token);

    const lines = await admin(
      "override",
      "teo-goulois",
      "2",
      "1",
      "early",
      "user",
      "--expires",
      "2099-01-31",
    );
    expect(lines).toContain(
      'Override: 5 serving machines, 10 URLs ("team") -> 2 serving machines, 1 URL ("early user"), expires 2099-01-31T00:00:00.000Z.',
    );
    // Only the runner's URL stops, but every serving machine of the owner rotates.
    expect(lines.some((line) => line.includes(`laptop (${laptop.machineId})`))).toBe(true);
    expect(lines.some((line) => line.includes(`runner (${runner.machineId})`))).toBe(true);
    // The releases happen; the next stop marks the machines again.
    context.sqlite.exec("UPDATE machines SET rotate_requested_at = NULL");
    expect(
      get("SELECT serving_machines, public_urls, reason, expires_at FROM plan_overrides"),
    ).toEqual({
      serving_machines: 2,
      public_urls: 1,
      reason: "early user",
      expires_at: Date.parse("2099-01-31"),
    });

    const removed = await admin("override", "--remove", "teo-goulois");
    expect(removed).toContain("Entitlement: 2 serving machines, 1 URL -> none.");
    expect(removed.some((line) => line.startsWith("Rotating laptop"))).toBe(true);
    expect(get("SELECT COUNT(*) AS n FROM plan_overrides")).toEqual({ n: 0 });
    expect(await admin("override", "--remove", "teo-goulois")).toEqual([
      expect.stringContaining("has no override"),
      "Nothing changed.",
    ]);
  });

  it("refuse malformed limits and dates", async () => {
    await signIn(context);
    await expect(admin("override", "teo-goulois", "-1", "10", "x")).rejects.toThrow(UsageError);
    await expect(admin("override", "teo-goulois", "2", "ten", "x")).rejects.toThrow(UsageError);
    await expect(admin("override", "teo-goulois", "2", "10")).rejects.toThrow(/reason/);
    await expect(
      admin("override", "teo-goulois", "2", "10", "x", "--expires", "soon"),
    ).rejects.toThrow(/needs a date/);
    await expect(
      admin("override", "teo-goulois", "2", "10", "x", "--expires", "2020-01-01"),
    ).rejects.toThrow(/already past/);
    expect(get("SELECT serving_machines FROM plan_overrides")).toEqual({ serving_machines: 5 });
  });
});

describe("the other commands", () => {
  it("queue a billing refresh", async () => {
    const { userId } = await signIn(context);
    await admin("refresh", "teo-goulois");
    const first = get<{ last: number; check_due_at: number }>(
      "SELECT last, check_due_at FROM billing_revisions WHERE user_id = ?",
      userId,
    );
    expect(first.last).toBe(0);
    expect(first.check_due_at).toBeGreaterThan(0);
  });

  it("show the account, its plan and the state of each URL", async () => {
    const { token, userId } = await signIn(context);
    const a = await createUrl(token, "a.localify");
    const b = await createUrl(token, "b.localify");
    context.sqlite
      .prepare("UPDATE plan_overrides SET public_urls = 1 WHERE user_id = ?")
      .run(userId);
    const lines = await admin("show", "teo-goulois");
    expect(lines).toContain("Plan: override. Entitlement: 5 serving machines, 1 URL.");
    expect(lines).toContain(`  ${a.slug}: serving on laptop`);
    expect(lines).toContain(`  ${b.slug}: suspended on laptop`);
  });

  it("say what is unknown or malformed", async () => {
    await expect(admin("ban", "nobody", "spam")).rejects.toThrow(AdminError);
    await expect(admin("takedown", "no-such-slug", "spam")).rejects.toThrow(/No public URL/);
    await expect(admin("ban", "nobody")).rejects.toThrow(UsageError);
    await expect(admin("launch")).rejects.toThrow(/Unknown command/);
    await expect(admin()).rejects.toThrow(/No command/);
  });
});

describe("an admin change that fails", () => {
  const ROTATION = /UPDATE machines SET rotate_requested_at/;

  it("applies nothing, and its retry writes the change with its rotation", async () => {
    const { token, machineId } = await signIn(context);
    await createUrl(token, "journal.localify");
    await serve(context, token);

    await expect(runCommand(adminDb(ROTATION), ["ban", "teo-goulois", "spam"])).rejects.toThrow(
      "D1 failed",
    );
    expect(get("SELECT banned_at FROM users")).toEqual({ banned_at: null });
    expect(machine(machineId).rotate_requested_at).toBeNull();

    const retry = await admin("ban", "teo-goulois", "spam");
    expect(retry.some((line) => line.startsWith("Rotating laptop"))).toBe(true);
    expect(
      get<{ banned_at: number | null }>("SELECT banned_at FROM users").banned_at,
    ).not.toBeNull();
    expect(machine(machineId).rotate_requested_at).not.toBeNull();
  });

  it("is repaired by a retry that finds the change already made", async () => {
    const { token, machineId } = await signIn(context);
    const bad = await createUrl(token, "bad.localify");
    await createUrl(token, "good.localify");
    await serve(context, token);
    // A takedown written without its rotation.
    context.sqlite.prepare("UPDATE public_urls SET taken_down_at = 1 WHERE id = ?").run(bad.id);

    const retry = await admin("takedown", bad.slug, "malware");
    expect(retry[0]).toContain("was already taken down");
    expect(retry.some((line) => line.startsWith("Rotating laptop"))).toBe(true);
    expect(machine(machineId).rotate_requested_at).not.toBeNull();
    expect(
      get<{ suspended_at: number | null }>(
        "SELECT suspended_at FROM public_urls WHERE id = ?",
        bad.id,
      ).suspended_at,
    ).not.toBeNull();
    expect(await admin("takedown", bad.slug, "malware")).toEqual([
      expect.stringContaining("was already taken down"),
      "Nothing changed.",
    ]);
  });
});

describe("rotations resolved at write time", () => {
  it("reach a machine that claimed the URL after the command read it", async () => {
    const laptop = await signIn(context, "teo-goulois", 1, "laptop");
    const runner = await signIn(context, "teo-goulois", 1, "runner");
    await createUrl(laptop.token, "a.localify");
    const c = await createUrl(laptop.token, "c.localify");
    await createUrl(runner.token, "b.localify");
    await serve(context, laptop.token);
    await serve(context, runner.token);

    // The command reads C on the laptop; the runner claims it before the write.
    const claim = () =>
      context.sqlite
        .prepare("UPDATE public_urls SET machine_id = ? WHERE id = ?")
        .run(runner.machineId, c.id);
    await runCommand(adminDb(undefined, claim), ["takedown", c.slug, "malware"]);
    expect(machine(laptop.machineId).rotate_requested_at).not.toBeNull();
    expect(machine(runner.machineId).rotate_requested_at).not.toBeNull();
  });

  it("reach the machine of a URL taken down after its owner deleted it", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const { token, machineId } = await signIn(context);
    const a = await createUrl(token, "a.localify");
    await createUrl(token, "b.localify");
    await serve(context, token);
    // Deleting A leaves the tunnel to B, and A's open streams on it.
    expect((await api(context, "DELETE", `/v1/public-urls/${a.id}`, { token })).status).toBe(204);
    await settle(context);
    expect(machine(machineId)).toMatchObject({ tunnel_state: "ready", rotate_requested_at: null });

    const lines = await admin("takedown", a.slug, "phishing");
    expect(lines.some((line) => line.startsWith("Rotating laptop"))).toBe(true);
    expect(machine(machineId).rotate_requested_at).not.toBeNull();
    await runSweep(context.env);
    expect(machine(machineId).tunnel_state).toBe("none");
  });
});

describe("a suspension waiting for its reconcile", () => {
  /**
   * Quota 1: A serves from the laptop, B and C from the runner are marked
   * suspended. A is deleted, and the reconcile that would resume B fails.
   */
  async function deleteWithoutResume() {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const laptop = await signIn(context, "teo-goulois", 1, "laptop");
    const runner = await signIn(context, "teo-goulois", 1, "runner");
    const a = await createUrl(laptop.token, "a.localify");
    const b = await createUrl(runner.token, "b.localify");
    await createUrl(runner.token, "c.localify");
    await admin("override", "teo-goulois", "2", "1", "team");
    await serve(context, laptop.token);
    await runSweep(context.env);
    context.sqlite
      .prepare("UPDATE public_urls SET deleted_at = ? WHERE id = ?")
      .run(Date.now(), a.id);
    return { laptop, runner, b };
  }

  function suspendedAt(id: string): number | null {
    return get<{ suspended_at: number | null }>(
      "SELECT suspended_at FROM public_urls WHERE id = ?",
      id,
    ).suspended_at;
  }

  it("keeps the URL from serving, so a takedown has nothing left to rotate", async () => {
    const { runner, b } = await deleteWithoutResume();
    // B ranks within the limit now, but stays suspended until a reconcile.
    const runtime = await serve(context, runner.token);
    expect(runtime.body.state).toBe("idle");
    expect(
      (runtime.body.publicUrls as { id: string; state: string }[]).find((url) => url.id === b.id),
    ).toMatchObject({ state: "suspended" });
    expect((await visit(context, `https://${b.slug}.localify.fyi/`)).status).toBe(402);
    expect(machine(runner.machineId).tunnel_state).toBe("none");

    // Nothing served B since its mark: skipping its rotation leaves no stream open.
    const lines = await admin("takedown", b.slug, "malware");
    expect(lines.some((line) => line.startsWith("Rotating"))).toBe(false);
    expect(machine(runner.machineId).tunnel_state).toBe("none");
  });

  it("serves the URL once the next reconcile resumes it, and then a takedown rotates", async () => {
    const { runner, b } = await deleteWithoutResume();
    await runSweep(context.env);
    expect(suspendedAt(b.id)).toBeNull();
    await serve(context, runner.token);
    expect(machine(runner.machineId).tunnel_state).toBe("ready");

    const lines = await admin("takedown", b.slug, "malware");
    expect(lines.some((line) => line.startsWith("Rotating runner"))).toBe(true);
    expect(machine(runner.machineId).rotate_requested_at).not.toBeNull();
  });
});
