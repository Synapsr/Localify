// The dashboard's HTML, rendered on the server from a `Dashboard`. Every
// value from D1 goes through `esc`; the only script is inline, behind the
// page's nonce, and sorts, filters and switches the theme in the browser.
// Nothing here links to an action: the dashboard only reads.

import type { PlanStatus } from "../../src/entitlement.ts";

import {
  PLAN_STATUSES,
  nearLimit,
  type Attention,
  type Dashboard,
  type UrlState,
  type UserView,
} from "./model.ts";

/** What every page shows of the read behind it: `readAt` is null before a first read. */
export type PageMeta = {
  source: "Production D1" | "Local D1";
  readAt: number | null;
  now: number;
  nonce: string;
  path: string;
};

export function esc(value: unknown): string {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function day(ms: number | null): string {
  return ms === null ? "—" : new Date(ms).toISOString().slice(0, 10);
}

function iso(ms: number): string {
  return `${new Date(ms).toISOString().slice(0, 19)}Z`;
}

/** "3 min ago", "2 days ago", "in 5 days". */
export function relative(ms: number, now: number): string {
  const seconds = Math.round((now - ms) / 1000);
  const abs = Math.abs(seconds);
  const [value, unit] =
    abs < 60
      ? [abs, "s"]
      : abs < 3600
        ? [Math.floor(abs / 60), "min"]
        : abs < 86400
          ? [Math.floor(abs / 3600), "h"]
          : [Math.floor(abs / 86400), abs < 2 * 86400 ? "day" : "days"];
  return seconds >= 0 ? `${value} ${unit} ago` : `in ${value} ${unit}`;
}

function when(ms: number | null, now: number): string {
  return ms === null
    ? `<span class="muted">—</span>`
    : `<time datetime="${iso(ms)}" title="${iso(ms)}">${esc(relative(ms, now))}</time>`;
}

const STATUS_LABELS: Record<PlanStatus, string> = {
  override: "Override",
  trialing: "Trialing",
  active: "Active",
  past_due: "Past due",
  canceling: "Canceling",
  none: "No plan",
};

function statusChip(status: PlanStatus): string {
  return `<span class="chip chip-${status}">${esc(STATUS_LABELS[status])}</span>`;
}

const URL_STATES: Record<UrlState, string> = {
  serving: "Serving",
  pending: "Suspended until the next reconcile",
  suspended: "Suspended",
  taken_down: "Taken down",
};

/**
 * A bar of `used` against `limit`, with the numbers in text: amber from 80%
 * of the limit, red above it, gray when the limit is 0 (nothing serves).
 */
function meter(used: number, limit: number): string {
  const share = limit > 0 ? Math.min(used / limit, 1) : used > 0 ? 1 : 0;
  const level =
    limit === 0 ? "none" : used > limit ? "over" : nearLimit(used, limit) ? "near" : "ok";
  const label =
    level === "over" ? " over the limit" : level === "near" ? " near or at the limit" : "";
  return `<span class="meter meter-${level}" title="${used} of ${limit}${label}">
    <span class="meter-bar"><span style="width:${Math.round(share * 100)}%"></span></span>
    <span class="meter-text">${used} / ${limit}</span></span>`;
}

function planCell(user: UserView, now: number): string {
  if (user.override) {
    const o = user.override;
    const ends =
      o.expiresAt === null
        ? "no expiry"
        : o.expired
          ? `expired ${day(o.expiresAt)}`
          : `until ${day(o.expiresAt)}`;
    const chip = o.expired ? statusChip(user.status) : statusChip("override");
    return `${chip} <span class="sub">${o.expired ? "override " : ""}${esc(o.reason)} · ${esc(ends)}</span>${
      o.expiresAt !== null && !o.expired ? `<span class="sub">${when(o.expiresAt, now)}</span>` : ""
    }`;
  }
  const detail = [user.interval, user.endsAt ? `ends ${user.endsAt.slice(0, 10)}` : null]
    .filter(Boolean)
    .join(" · ");
  return `${statusChip(user.status)}${detail ? ` <span class="sub">${esc(detail)}</span>` : ""}`;
}

const STYLE = `
:root {
  color-scheme: light;
  --bg: #f6f6f4; --surface: #fcfcfb; --surface-2: #f0efec; --border: #e2e1dc;
  --text: #0b0b0b; --text-2: #52514e; --muted: #7a7974;
  --accent: #2a78d6; --accent-soft: #dce9f9;
  --warn: #b26b00; --warn-soft: #fbecd0; --bad: #c8302f; --bad-soft: #f9dcdc;
  --good: #11804f; --good-soft: #d6f0e3;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    color-scheme: dark;
    --bg: #121211; --surface: #1a1a19; --surface-2: #232321; --border: #33332f;
    --text: #f0efec; --text-2: #c3c2b7; --muted: #8f8e86;
    --accent: #3987e5; --accent-soft: #1d3450;
    --warn: #e0a030; --warn-soft: #3d2f12; --bad: #e66767; --bad-soft: #432222;
    --good: #3cc58a; --good-soft: #173528;
  }
}
:root[data-theme="dark"] {
  color-scheme: dark;
  --bg: #121211; --surface: #1a1a19; --surface-2: #232321; --border: #33332f;
  --text: #f0efec; --text-2: #c3c2b7; --muted: #8f8e86;
  --accent: #3987e5; --accent-soft: #1d3450;
  --warn: #e0a030; --warn-soft: #3d2f12; --bad: #e66767; --bad-soft: #432222;
  --good: #3cc58a; --good-soft: #173528;
}
* { box-sizing: border-box; }
html { -webkit-text-size-adjust: 100%; }
body {
  margin: 0; background: var(--bg); color: var(--text);
  font: 15px/1.45 ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif;
}
main { max-width: 1180px; margin: 0 auto; padding: 16px 16px 48px; }
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
h1 { font-size: 20px; margin: 0; }
h2 { font-size: 15px; margin: 0 0 12px; color: var(--text-2); font-weight: 600; }
header { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 16px; margin-bottom: 16px; }
header .meta { color: var(--text-2); font-size: 13px; flex: 1 1 auto; }
.actions { display: flex; gap: 8px; }
.button {
  display: inline-flex; align-items: center; gap: 6px; min-height: 36px; padding: 0 12px;
  border: 1px solid var(--border); border-radius: 8px; background: var(--surface);
  color: var(--text); font: inherit; font-size: 14px; cursor: pointer;
}
.button:hover { background: var(--surface-2); text-decoration: none; }
.badge { font-size: 12px; color: var(--text-2); border: 1px solid var(--border); border-radius: 999px; padding: 1px 8px; }
section { background: var(--surface); border: 1px solid var(--border); border-radius: 12px; padding: 16px; margin-bottom: 16px; }
.tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 12px; margin-bottom: 16px; }
.tile { background: var(--surface); border: 1px solid var(--border); border-radius: 12px; padding: 12px 14px; }
.tile .label { color: var(--text-2); font-size: 13px; }
.tile .value { font-size: 26px; font-weight: 650; font-variant-numeric: tabular-nums; line-height: 1.2; }
.tile .sub { color: var(--muted); font-size: 12px; }
.row { display: flex; flex-wrap: wrap; gap: 8px; }
.chip {
  display: inline-block; border-radius: 999px; padding: 1px 8px; font-size: 12px; font-weight: 600;
  background: var(--surface-2); color: var(--text-2); white-space: nowrap;
}
.chip-override { background: var(--accent-soft); color: var(--accent); }
.chip-trialing, .chip-active { background: var(--good-soft); color: var(--good); }
.chip-past_due, .chip-bad { background: var(--bad-soft); color: var(--bad); }
.chip-canceling, .chip-warn { background: var(--warn-soft); color: var(--warn); }
.status-count { display: inline-flex; align-items: center; gap: 6px; padding: 6px 10px; border: 1px solid var(--border); border-radius: 8px; }
.status-count b { font-variant-numeric: tabular-nums; }
.sub { display: block; color: var(--muted); font-size: 12px; }
.muted { color: var(--muted); }
.chart svg { display: block; width: 100%; height: auto; max-height: 240px; }
.chart .narrow { display: none; }
.chart .bar { fill: var(--accent); }
.chart .bar:hover { opacity: .8; }
.chart .axis { fill: var(--muted); font-size: 11px; }
.chart .grid { stroke: var(--border); stroke-width: 1; }
.attention { list-style: none; margin: 0; padding: 0; }
.attention li { display: flex; flex-wrap: wrap; gap: 4px 10px; align-items: baseline; padding: 8px 0; border-top: 1px solid var(--border); }
.attention li:first-child { border-top: 0; }
.toolbar { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; margin-bottom: 12px; }
.toolbar h2 { margin: 0; flex: 1 1 auto; }
input[type=search] {
  font: inherit; font-size: 16px; min-height: 36px; padding: 0 10px; width: min(100%, 280px);
  border: 1px solid var(--border); border-radius: 8px; background: var(--bg); color: var(--text);
}
.scroll { overflow-x: auto; -webkit-overflow-scrolling: touch; margin: 0 -16px; padding: 0 16px; }
table { border-collapse: collapse; width: 100%; font-size: 14px; }
th, td { text-align: left; padding: 8px 10px; border-top: 1px solid var(--border); vertical-align: top; }
th { color: var(--text-2); font-weight: 600; font-size: 12px; white-space: nowrap; border-top: 0; }
th button { all: unset; cursor: pointer; }
th button::after { content: " ↕"; color: var(--muted); }
th[aria-sort=ascending] button::after { content: " ↑"; color: var(--text); }
th[aria-sort=descending] button::after { content: " ↓"; color: var(--text); }
td.num { font-variant-numeric: tabular-nums; white-space: nowrap; }
.meter { display: inline-flex; align-items: center; gap: 8px; white-space: nowrap; }
.meter-bar { display: inline-block; width: 64px; height: 6px; border-radius: 3px; background: var(--surface-2); overflow: hidden; }
.meter-bar span { display: block; height: 100%; border-radius: 3px; background: var(--accent); }
.meter-near .meter-bar span { background: var(--warn); }
.meter-over .meter-bar span { background: var(--bad); }
.meter-none .meter-bar span { background: var(--muted); }
.meter-text { font-variant-numeric: tabular-nums; font-size: 13px; }
dl { display: grid; grid-template-columns: max-content 1fr; gap: 6px 16px; margin: 0; }
dt { color: var(--text-2); }
dd { margin: 0; overflow-wrap: anywhere; }
.grid-2 { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 16px; }
.grid-2 > section { margin-bottom: 0; }
.items { list-style: none; margin: 0; padding: 0; }
.items li { padding: 10px 0; border-top: 1px solid var(--border); display: flex; flex-wrap: wrap; justify-content: space-between; gap: 4px 12px; }
.items li:first-child { border-top: 0; }
.mono { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 13px; overflow-wrap: anywhere; }
@media (max-width: 640px) {
  .chart .wide { display: none; }
  .chart .narrow { display: block; }
  .tile .value { font-size: 22px; }
  dl { grid-template-columns: 1fr; gap: 2px; }
  dd { margin-bottom: 8px; }
}
`;

// Applies the saved theme before the first paint, sorts and filters the
// users table, and drops `?reload` from the address bar once served.
const SCRIPT = `
(() => {
  const root = document.documentElement;
  const saved = (() => { try { return localStorage.getItem("theme"); } catch { return null; } })();
  if (saved === "light" || saved === "dark") root.dataset.theme = saved;
  if (location.search.includes("reload")) history.replaceState(null, "", location.pathname);
  addEventListener("DOMContentLoaded", () => {
    document.getElementById("theme")?.addEventListener("click", () => {
      const dark = root.dataset.theme
        ? root.dataset.theme === "dark"
        : matchMedia("(prefers-color-scheme: dark)").matches;
      root.dataset.theme = dark ? "light" : "dark";
      try { localStorage.setItem("theme", root.dataset.theme); } catch {}
    });
    const table = document.getElementById("users");
    if (!table) return;
    const body = table.tBodies[0];
    const rows = [...body.rows];
    const search = document.getElementById("search");
    const count = document.getElementById("count");
    const filter = () => {
      const q = search.value.trim().toLowerCase();
      let shown = 0;
      for (const row of rows) {
        const match = !q || row.dataset.search.includes(q);
        row.hidden = !match;
        if (match) shown++;
      }
      count.textContent = shown === rows.length ? rows.length + " accounts" : shown + " of " + rows.length;
    };
    search.addEventListener("input", filter);
    for (const th of table.tHead.rows[0].cells) {
      const button = th.querySelector("button");
      if (!button) continue;
      button.addEventListener("click", () => {
        const key = th.dataset.key;
        const type = th.dataset.type;
        const dir = th.getAttribute("aria-sort") === "ascending" ? -1 : 1;
        for (const other of table.tHead.rows[0].cells) other.removeAttribute("aria-sort");
        th.setAttribute("aria-sort", dir === 1 ? "ascending" : "descending");
        const value = (row) => row.dataset[key] ?? "";
        rows.sort((a, b) => {
          const x = value(a), y = value(b);
          return dir * (type === "num" ? (Number(x) || 0) - (Number(y) || 0) : x.localeCompare(y));
        });
        body.append(...rows);
      });
    }
  });
})();
`;

function layout(title: string, meta: PageMeta, content: string): string {
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<meta name="referrer" content="no-referrer">
<title>${esc(title)}</title>
<style>${STYLE}</style>
<script nonce="${meta.nonce}">${SCRIPT}</script>
</head>
<body>
<main>
<header>
  <h1><a href="/" style="color:inherit">Localify Cloud</a></h1>
  <span class="badge" title="This dashboard only reads D1">Read-only</span>
  <span class="meta">${esc(meta.source)}${
    meta.readAt === null
      ? ", not read yet"
      : `, read ${when(meta.readAt, meta.now)} · ${esc(iso(meta.readAt).slice(11, 19))} UTC`
  }</span>
  <span class="actions">
    <a class="button" href="${esc(meta.path)}?reload=1" title="Read D1 again">↻ Refresh</a>
    <button class="button" id="theme" type="button" title="Light or dark">◐ Theme</button>
  </span>
</header>
${content}
</main>
</body>
</html>`;
}

function tile(label: string, value: number, sub = ""): string {
  return `<div class="tile"><div class="label">${esc(label)}</div><div class="value">${value}</div>${
    sub ? `<div class="sub">${sub}</div>` : ""
  }</div>`;
}

/**
 * Sign-ups per week as SVG bars, one series: no legend, a tooltip per bar.
 * The last `count` weeks, drawn `width` units wide; text keeps its size
 * when the SVG is drawn about that many pixels wide.
 */
export function signupChart(weeks: Dashboard["signupsByWeek"], count = 26, width = 720): string {
  if (weeks.length === 0) return `<p class="muted">No sign-ups yet.</p>`;
  const shown = weeks.slice(-count);
  const height = 180;
  const left = 28;
  const bottom = 22;
  const top = 8;
  const max = Math.max(1, ...shown.map((week) => week.count));
  const ticks = max <= 4 ? max : 4;
  const plotHeight = height - top - bottom;
  const step = (width - left) / shown.length;
  const barWidth = Math.max(2, Math.min(16, step - 4));
  const grid = Array.from({ length: ticks + 1 }, (_, i) => {
    const value = Math.round((max * i) / ticks);
    const y = top + plotHeight - (value / max) * plotHeight;
    return `<line class="grid" x1="${left}" x2="${width}" y1="${y.toFixed(1)}" y2="${y.toFixed(1)}"/>
      <text class="axis" x="${left - 6}" y="${(y + 4).toFixed(1)}" text-anchor="end">${value}</text>`;
  }).join("");
  const labelEvery = Math.ceil(shown.length / 6);
  const bars = shown
    .map((week, i) => {
      const h = (week.count / max) * plotHeight;
      const x = left + i * step + (step - barWidth) / 2;
      const y = top + plotHeight - h;
      const label = `Week of ${day(week.weekStart)}: ${week.count} ${week.count === 1 ? "sign-up" : "sign-ups"}`;
      // A hit target as tall as the plot, so an empty week can be hovered too.
      return `<g><title>${esc(label)}</title>
        <rect x="${(left + i * step).toFixed(1)}" y="${top}" width="${step.toFixed(1)}" height="${plotHeight}" fill="transparent"/>
        ${week.count > 0 ? `<rect class="bar" x="${x.toFixed(1)}" y="${y.toFixed(1)}" width="${barWidth.toFixed(1)}" height="${h.toFixed(1)}" rx="${Math.min(4, barWidth / 2)}"/>` : ""}
        ${i % labelEvery === 0 ? `<text class="axis" x="${(x + barWidth / 2).toFixed(1)}" y="${height - 6}" text-anchor="middle">${esc(day(week.weekStart).slice(5))}</text>` : ""}</g>`;
    })
    .join("");
  return `<svg viewBox="0 0 ${width} ${height}" role="img" aria-label="New sign-ups per week, the last ${shown.length} weeks">${grid}${bars}</svg>`;
}

const ATTENTION_LABELS: Record<Attention["kind"], [string, string]> = {
  banned: ["Banned", "chip-bad"],
  past_due: ["Past due", "chip-bad"],
  override_expiring: ["Override ending", "chip-warn"],
  urls: ["URLs near limit", "chip-warn"],
  machines: ["Machines near limit", "chip-warn"],
};

function attentionList(items: Attention[]): string {
  if (items.length === 0) return `<p class="muted">Nothing needs attention.</p>`;
  return `<ul class="attention">${items
    .map((item) => {
      const [label, chip] = ATTENTION_LABELS[item.kind];
      return `<li><span class="chip ${chip}">${esc(label)}</span>
        <a href="/users/${encodeURIComponent(item.userId)}">${esc(item.login)}</a>
        <span class="muted">${esc(item.detail)}</span></li>`;
    })
    .join("")}</ul>`;
}

function userRow(user: UserView, now: number): string {
  const search = [user.login, user.githubId, user.status, user.override?.reason ?? ""]
    .join(" ")
    .toLowerCase();
  return `<tr data-search="${esc(search)}" data-login="${esc(user.login.toLowerCase())}"
      data-created="${user.createdAt}" data-plan="${esc(user.status)}"
      data-machines="${user.servingSlots}" data-urls="${user.liveUrls}"
      data-activity="${user.lastActivityAt ?? 0}" data-banned="${user.bannedAt === null ? 0 : 1}">
    <td><a href="/users/${encodeURIComponent(user.id)}">${esc(user.login)}</a></td>
    <td class="num">${esc(day(user.createdAt))}</td>
    <td>${planCell(user, now)}</td>
    <td>${meter(user.servingSlots, user.entitlement.servingMachines)}<span class="sub">${user.signedIn} signed in of ${user.signedInCap}</span></td>
    <td>${meter(user.liveUrls, user.entitlement.publicUrls)}<span class="sub">${user.servingUrls} serving</span></td>
    <td class="num">${when(user.lastActivityAt, now)}</td>
    <td>${user.bannedAt === null ? "" : `<span class="chip chip-bad" title="${esc(user.bannedReason ?? "")}">Banned</span>`}</td>
  </tr>`;
}

export function overviewPage(dashboard: Dashboard, meta: PageMeta): string {
  const t = dashboard.totals;
  const statuses = PLAN_STATUSES.map(
    (status) =>
      `<span class="status-count">${statusChip(status)} <b>${dashboard.statuses[status]}</b></span>`,
  ).join("");
  const headers: [string, string, "num" | "text", string][] = [
    ["login", "GitHub login", "text", ""],
    ["created", "Signed up", "num", ""],
    ["plan", "Plan or override", "text", ""],
    ["machines", "Serving machines", "num", ""],
    ["urls", "Live URLs", "num", ""],
    [
      "activity",
      "Last activity",
      "num",
      "Latest sign-in, tunnel start, URL change or billing event: D1 records no visits",
    ],
    ["banned", "Banned", "num", ""],
  ];
  const content = `
<div class="tiles">
  ${tile("Users", t.users, `+${t.newLast7Days} in 7 days · +${t.newLast30Days} in 30`)}
  ${tile("Active machines", t.activeMachines, `${t.servingMachines} holding a serving slot`)}
  ${tile("Live URLs", t.liveUrls, `${t.servingUrls} serving`)}
  ${tile("Add-on units", dashboard.addonUnits.machines + dashboard.addonUnits.urlPacks, `${dashboard.addonUnits.machines} machine · ${dashboard.addonUnits.urlPacks} URL packs`)}
  ${tile("Banned", t.banned)}
  ${tile("Takedowns", t.takedowns)}
</div>
<section><h2>Accounts per plan</h2><div class="row">${statuses}</div></section>
<section class="chart"><h2>New sign-ups per week</h2><div class="wide">${signupChart(dashboard.signupsByWeek)}</div><div class="narrow">${signupChart(dashboard.signupsByWeek, 12, 360)}</div></section>
<section><h2>Needs attention</h2>${attentionList(dashboard.attention)}</section>
<section>
  <div class="toolbar"><h2>Users <span class="muted" id="count">${dashboard.users.length} accounts</span></h2>
    <input type="search" id="search" placeholder="Search login, id, plan" aria-label="Search users" autocomplete="off"></div>
  <div class="scroll"><table id="users">
    <thead><tr>${headers
      .map(
        ([key, label, type, title]) =>
          `<th data-key="${key}" data-type="${type}"${key === "created" ? ` aria-sort="descending"` : ""}${title ? ` title="${esc(title)}"` : ""}><button type="button">${esc(label)}</button></th>`,
      )
      .join("")}</tr></thead>
    <tbody>${dashboard.users.map((user) => userRow(user, meta.now)).join("")}</tbody>
  </table></div>
</section>`;
  return layout("Localify Cloud", meta, content);
}

export function userPage(user: UserView, meta: PageMeta): string {
  const now = meta.now;
  const e = user.entitlement;
  const override = user.override
    ? `${user.override.servingMachines} serving machines, ${user.override.publicUrls} URLs, ${
        user.override.remoteAccess ? "remote access" : "no remote access"
      } · "${esc(user.override.reason)}" · ${
        user.override.expiresAt === null
          ? "no expiry"
          : `${user.override.expired ? "expired" : "expires"} ${esc(iso(user.override.expiresAt))}`
      }`
    : `<span class="muted">None</span>`;
  const billing =
    user.billingRefreshedAt === null
      ? `<span class="muted">No billing snapshot</span>`
      : `Pro ${esc(user.proStatus ?? "none")}${user.interval ? ` (${esc(user.interval)})` : ""} · snapshot ${when(user.billingRefreshedAt, now)}`;
  const dates = [
    user.trialEndsAt ? `trial ends ${user.trialEndsAt.slice(0, 10)}` : null,
    user.renewsAt ? `renews ${user.renewsAt.slice(0, 10)}` : null,
    user.endsAt ? `ends ${user.endsAt.slice(0, 10)}` : null,
  ].filter(Boolean);
  const machines = user.machines.length
    ? `<ul class="items">${user.machines
        .map(
          (machine) => `<li><span><b>${esc(machine.name)}</b>
            <span class="sub">signed in ${esc(day(machine.createdAt))}${machine.linked ? " · remote key" : ""}</span></span>
            <span>${
              machine.status !== "active"
                ? `<span class="chip">${esc(machine.status)}</span>`
                : machine.serving
                  ? `<span class="chip chip-active">Serving slot · ${esc(machine.tunnelState)}</span>`
                  : `<span class="chip">No slot</span>`
            }</span></li>`,
        )
        .join("")}</ul>`
    : `<p class="muted">No signed-in machine.</p>`;
  const urlChip: Record<UrlState, string> = {
    serving: "chip-active",
    pending: "chip-warn",
    suspended: "chip-warn",
    taken_down: "chip-bad",
  };
  const urls = user.urls.length
    ? `<ul class="items">${user.urls
        .map(
          (
            url,
          ) => `<li><span><span class="mono">${esc(url.slug)}</span>${url.wildcard ? ` <span class="chip">wildcard</span>` : ""}${url.protected ? "" : ` <span class="chip chip-warn">open</span>`}
            <span class="sub">${url.machineName ? `on ${esc(url.machineName)}` : "no machine"} · created ${esc(day(url.createdAt))}${
              url.takenDownAt !== null
                ? ` · taken down ${esc(day(url.takenDownAt))}: ${esc(url.takenDownReason ?? "(no reason)")}`
                : ""
            }${url.deleted ? " · deleted" : ""}</span></span>
            <span><span class="chip ${urlChip[url.state]}">${esc(URL_STATES[url.state])}</span></span></li>`,
        )
        .join("")}</ul>`
    : `<p class="muted">No public URL.</p>`;
  const content = `
<p><a href="/">← All accounts</a></p>
<section>
  <div class="toolbar"><h2 style="color:var(--text);font-size:18px">${esc(user.login)}</h2>${statusChip(user.status)}${
    user.bannedAt !== null ? ` <span class="chip chip-bad">Banned</span>` : ""
  }</div>
  <dl>
    <dt>GitHub id</dt><dd class="mono">${user.githubId}</dd>
    <dt>User id</dt><dd class="mono">${esc(user.id)}</dd>
    <dt>Signed up</dt><dd>${esc(iso(user.createdAt))} · ${when(user.createdAt, now)}</dd>
    <dt>Last activity</dt><dd>${when(user.lastActivityAt, now)}</dd>
    <dt>Ban</dt><dd>${user.bannedAt === null ? `<span class="muted">Not banned</span>` : `${esc(iso(user.bannedAt))}: ${esc(user.bannedReason ?? "(no reason)")}`}</dd>
    <dt>Override</dt><dd>${override}</dd>
    <dt>Billing</dt><dd>${billing}${dates.length ? `<span class="sub">${esc(dates.join(" · "))}</span>` : ""}</dd>
    <dt>Add-ons counting</dt><dd>${user.addons.machines} machine units, ${user.addons.urlPacks} URL packs</dd>
    <dt>Entitlement</dt><dd>${
      e.servingMachines === 0 && e.publicUrls === 0 && !e.remoteAccess
        ? "none"
        : `${e.servingMachines} serving machines, ${e.publicUrls} URLs, ${e.remoteAccess ? "remote access" : "no remote access"}`
    }</dd>
    <dt>Serving machines</dt><dd>${meter(user.servingSlots, e.servingMachines)}</dd>
    <dt>Signed-in machines</dt><dd>${meter(user.signedIn, user.signedInCap)}</dd>
    <dt>Live URLs</dt><dd>${meter(user.liveUrls, e.publicUrls)} <span class="muted">${user.servingUrls} serving</span></dd>
  </dl>
</section>
<div class="grid-2">
  <section><h2>Machines</h2>${machines}</section>
  <section><h2>Public URLs</h2>${urls}</section>
</div>`;
  return layout(`${user.login} · Localify Cloud`, meta, content);
}

export function messagePage(title: string, message: string, meta: PageMeta): string {
  return layout(
    `${title} · Localify Cloud`,
    meta,
    `<section><h2>${esc(title)}</h2><p>${esc(message)}</p></section>`,
  );
}
