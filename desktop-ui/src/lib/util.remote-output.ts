import type { RemoteOutputDataDto } from "@/types/localify";

// The Output panel's lines (docs/remote-machines.md, "Showing it"): plain text from the
// target, at most 10,000 lines, the oldest dropped first.

export const MAX_OUTPUT_LINES = 10_000;

export interface OutputEntry {
  /** Stable for React, across trimming. */
  key: number;
  text: string;
  /** A line of the service's, or something the panel says between them. */
  kind: "line" | "notice";
}

export interface OutputBuffer {
  entries: OutputEntry[];
  /** A line still coming in pieces (`cut`), shown last. */
  partial: string | null;
  nextKey: number;
}

export function emptyOutput(): OutputBuffer {
  return { entries: [], partial: null, nextKey: 0 };
}

function push(buffer: OutputBuffer, text: string, kind: OutputEntry["kind"]): void {
  buffer.entries.push({ key: buffer.nextKey, text, kind });
  buffer.nextKey += 1;
}

function trim(buffer: OutputBuffer, limit: number): OutputBuffer {
  if (buffer.entries.length > limit) {
    buffer.entries = buffer.entries.slice(buffer.entries.length - limit);
  }
  return buffer;
}

/** Adds one `output.data`: its lines, joined where a line came in pieces. */
export function appendOutput(
  current: OutputBuffer,
  data: Pick<RemoteOutputDataDto, "lines" | "rotated" | "ended">,
  limit: number = MAX_OUTPUT_LINES,
): OutputBuffer {
  const buffer: OutputBuffer = { ...current, entries: [...current.entries] };
  if (data.rotated) push(buffer, "Earlier output was dropped.", "notice");
  for (const line of data.lines) {
    const text = (buffer.partial ?? "") + line.text;
    if (line.cut) {
      buffer.partial = text;
    } else {
      buffer.partial = null;
      push(buffer, text, "line");
    }
  }
  if (data.ended && buffer.partial !== null) {
    push(buffer, buffer.partial, "line");
    buffer.partial = null;
  }
  return trim(buffer, limit);
}

/** Adds something the panel says, between the service's lines. */
export function appendNotice(
  current: OutputBuffer,
  text: string,
  limit: number = MAX_OUTPUT_LINES,
): OutputBuffer {
  const buffer: OutputBuffer = { ...current, entries: [...current.entries] };
  if (buffer.partial !== null) {
    push(buffer, buffer.partial, "line");
    buffer.partial = null;
  }
  push(buffer, text, "notice");
  return trim(buffer, limit);
}

/** The service's lines that contain `query`, ignoring case; every entry without one. */
export function filterOutput(entries: OutputEntry[], query: string): OutputEntry[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return entries;
  return entries.filter(
    (entry) => entry.kind === "line" && entry.text.toLowerCase().includes(needle),
  );
}
