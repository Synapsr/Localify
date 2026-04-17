import type { DomainInfoDto } from "@/types/roxy";

export type RequestKind = "completed" | "no-route" | "domain-not-registered";

export interface RequestEntry {
  id: number;
  timestamp: Date;
  timestampRaw: string;
  kind: RequestKind;
  method: string | null;
  host: string;
  path: string | null;
  status: number;
  projectId: string | null;
  projectName: string | null;
}

const HEAD_RE =
  /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z?)\s+(TRACE|DEBUG|INFO|WARN|ERROR)\b\s*(.*)$/;

const FIELD_RE = /(\w+)=("(?:[^"\\]|\\.)*"|\S+)/g;

interface Fields {
  method?: string;
  host?: string;
  path?: string;
  status?: string;
}

function parseFields(rest: string): { message: string; fields: Fields } {
  const fields: Fields = {};
  let m: RegExpExecArray | null;
  FIELD_RE.lastIndex = 0;
  let lastIndex = 0;
  const message = [] as string[];
  let cursor = 0;
  const parts: Array<{ type: "text" | "field"; start: number; end: number }> = [];

  while ((m = FIELD_RE.exec(rest)) !== null) {
    if (m.index > cursor) {
      parts.push({ type: "text", start: cursor, end: m.index });
    }
    parts.push({ type: "field", start: m.index, end: m.index + m[0].length });
    const key = m[1];
    let value = m[2];
    if (value.startsWith('"') && value.endsWith('"')) {
      value = value.slice(1, -1).replace(/\\(.)/g, "$1");
    }
    if (key === "method" || key === "host" || key === "path" || key === "status") {
      fields[key] = value;
    }
    cursor = m.index + m[0].length;
    lastIndex = cursor;
  }
  if (lastIndex < rest.length) {
    parts.push({ type: "text", start: lastIndex, end: rest.length });
  }

  for (const p of parts) {
    if (p.type === "text") {
      message.push(rest.slice(p.start, p.end));
    }
  }

  return {
    message: message.join("").trim(),
    fields,
  };
}

function stripPort(host: string): string {
  const i = host.indexOf(":");
  return i === -1 ? host : host.slice(0, i);
}

function matchDomain(host: string, domains: DomainInfoDto[]): DomainInfoDto | null {
  const bare = stripPort(host).toLowerCase();
  let bestScore = -1;
  let bestMatch: DomainInfoDto | null = null;
  for (const d of domains) {
    const domain = d.registration.domain.toLowerCase();
    if (bare === domain) return d;
    if (d.registration.wildcard && bare.endsWith(`.${domain}`)) {
      const score = domain.length;
      if (score > bestScore) {
        bestScore = score;
        bestMatch = d;
      }
    }
  }
  return bestMatch;
}

export function parseRequests(content: string, domains: DomainInfoDto[]): RequestEntry[] {
  if (!content) return [];

  const lines = content.split("\n");
  const out: RequestEntry[] = [];

  for (const line of lines) {
    if (!line) continue;
    const head = HEAD_RE.exec(line);
    if (!head) continue;
    const [, timestampRaw, , rest] = head;
    const parsed = Date.parse(timestampRaw);
    if (Number.isNaN(parsed)) continue;

    const { message, fields } = parseFields(rest);

    let kind: RequestKind | null = null;
    if (message === "Request completed") kind = "completed";
    else if (message === "No route found") kind = "no-route";
    else if (message === "Domain not registered") kind = "domain-not-registered";
    if (kind === null) continue;
    if (!fields.host) continue;

    const match = matchDomain(fields.host, domains);
    const status =
      kind === "completed" && fields.status ? Number.parseInt(fields.status, 10) || 0 : 404;

    out.push({
      id: out.length,
      timestamp: new Date(parsed),
      timestampRaw,
      kind,
      method: fields.method ?? null,
      host: stripPort(fields.host),
      path: fields.path ?? null,
      status,
      projectId: match?.projectId ?? null,
      projectName: match?.projectName ?? null,
    });
  }

  return out;
}

export function formatClock(date: Date): string {
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}
