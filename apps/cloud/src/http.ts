// JSON responses and errors for the API.

/**
 * An error the API answers with `{"error", "message"}`, and `details`'s
 * fields beside them: extra fields only, so every client keeps parsing it.
 */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    readonly details: Record<string, unknown> = {},
  ) {
    super(message);
  }
}

export function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" },
  });
}

export function noContent(): Response {
  return new Response(null, { status: 204, headers: { "cache-control": "no-store" } });
}

export function errorResponse(error: ApiError): Response {
  return json({ ...error.details, error: error.code, message: error.message }, error.status);
}

const MAX_BODY_BYTES = 16 * 1024;

/**
 * The body as text, or `null` past `maxBytes`. Reading stops at the limit, so a
 * large body never sits in the isolate's memory, which every request shares.
 */
export async function readLimitedText(request: Request, maxBytes: number): Promise<string | null> {
  const bytes = await readLimitedBytes(request, maxBytes);
  return bytes === null ? null : new TextDecoder().decode(bytes);
}

/** The body as it came, byte for byte, or `null` past `maxBytes`, read like `readLimitedText`. */
export async function readLimitedBytes(
  request: Request,
  maxBytes: number,
): Promise<Uint8Array | null> {
  const declared = Number(request.headers.get("content-length"));
  if (Number.isFinite(declared) && declared > maxBytes) {
    await request.body?.cancel();
    return null;
  }
  if (!request.body) return new Uint8Array(0);
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > maxBytes) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
}

/** The request's JSON object body. A missing body reads as `{}`. */
export async function readJson(request: Request): Promise<Record<string, unknown>> {
  const text = await readLimitedText(request, MAX_BODY_BYTES);
  if (text === null) {
    throw new ApiError(413, "body_too_large", "The request body is too large.");
  }
  if (text.trim() === "") return {};
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    throw new ApiError(
      400,
      "invalid_json",
      "The request body is not valid JSON. Send a JSON object.",
    );
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new ApiError(400, "invalid_json", "The request body must be a JSON object.");
  }
  return value as Record<string, unknown>;
}
