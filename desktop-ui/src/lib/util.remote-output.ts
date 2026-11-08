import type { RemoteOutputDataDto } from "@/types/localify";

// The Output panel's lines (docs/remote-machines.md, "Showing it"): plain text from the
// target, at most 10,000 lines and 4 MiB of text, the oldest dropped first.

export const MAX_OUTPUT_LINES = 10_000;
/** The text the panel keeps, partial line included, in UTF-16 code units. */
export const MAX_OUTPUT_CHARS = 4 * 1024 * 1024;
/** A line arriving in pieces is shown, cut, once it grows this long. */
export const MAX_PARTIAL_CHARS = 64 * 1024;

export interface OutputEntry {
  /** Stable for React, across trimming. */
  key: number;
  text: string;
  /** A line of the service's, or something the panel says between them. */
  kind: "line" | "notice";
  /** A piece of a line too long to keep whole: it goes on in the next entry. */
  cut?: boolean;
}

export interface OutputBuffer {
  entries: OutputEntry[];
  /** A line still coming in pieces (`cut`), shown last. */
  partial: string | null;
  nextKey: number;
  /** The text of `entries` and `partial`, in UTF-16 code units. */
  chars: number;
}

export interface OutputLimits {
  lines: number;
  chars: number;
  partial: number;
}

const LIMITS: OutputLimits = {
  lines: MAX_OUTPUT_LINES,
  chars: MAX_OUTPUT_CHARS,
  partial: MAX_PARTIAL_CHARS,
};

export function emptyOutput(): OutputBuffer {
  return { entries: [], partial: null, nextKey: 0, chars: 0 };
}

function push(buffer: OutputBuffer, text: string, kind: OutputEntry["kind"], cut = false): void {
  buffer.entries.push(
    cut ? { key: buffer.nextKey, text, kind, cut } : { key: buffer.nextKey, text, kind },
  );
  buffer.nextKey += 1;
  buffer.chars += text.length;
}

/** Drops the oldest entries past either bound, the partial line counted. */
function trim(buffer: OutputBuffer, limits: OutputLimits): OutputBuffer {
  let drop = 0;
  while (
    drop < buffer.entries.length &&
    (buffer.entries.length - drop > limits.lines || buffer.chars > limits.chars)
  ) {
    buffer.chars -= buffer.entries[drop].text.length;
    drop += 1;
  }
  if (drop > 0) buffer.entries = buffer.entries.slice(drop);
  return buffer;
}

function setPartial(buffer: OutputBuffer, text: string | null): void {
  buffer.chars += (text?.length ?? 0) - (buffer.partial?.length ?? 0);
  buffer.partial = text;
}

/** Adds one `output.data`: its lines, joined where a line came in pieces. */
export function appendOutput(
  current: OutputBuffer,
  data: Pick<RemoteOutputDataDto, "lines" | "rotated" | "ended">,
  limits: Partial<OutputLimits> = {},
): OutputBuffer {
  const bounds = { ...LIMITS, ...limits };
  const buffer: OutputBuffer = { ...current, entries: [...current.entries] };
  if (data.rotated) push(buffer, "Earlier output was dropped.", "notice");
  for (const line of data.lines) {
    let text = (buffer.partial ?? "") + line.text;
    setPartial(buffer, null);
    // A line that never ends is kept in pieces, each one an entry.
    while (text.length > bounds.partial) {
      push(buffer, text.slice(0, bounds.partial), "line", true);
      text = text.slice(bounds.partial);
    }
    if (line.cut) setPartial(buffer, text);
    else push(buffer, text, "line");
  }
  if (data.ended && buffer.partial !== null) {
    const rest = buffer.partial;
    setPartial(buffer, null);
    push(buffer, rest, "line");
  }
  return trim(buffer, bounds);
}

/** Adds something the panel says, between the service's lines. */
export function appendNotice(
  current: OutputBuffer,
  text: string,
  limits: Partial<OutputLimits> = {},
): OutputBuffer {
  const buffer: OutputBuffer = { ...current, entries: [...current.entries] };
  if (buffer.partial !== null) {
    const rest = buffer.partial;
    setPartial(buffer, null);
    push(buffer, rest, "line");
  }
  push(buffer, text, "notice");
  return trim(buffer, { ...LIMITS, ...limits });
}

/** The service's lines that contain `query`, ignoring case; every entry without one. */
export function filterOutput(entries: OutputEntry[], query: string): OutputEntry[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return entries;
  return entries.filter(
    (entry) => entry.kind === "line" && entry.text.toLowerCase().includes(needle),
  );
}
