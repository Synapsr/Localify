// JSON responses and errors for the API.

/** An error the API answers with `{"error", "message"}`. */
export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
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
  return json({ error: error.code, message: error.message }, error.status);
}

const MAX_BODY_BYTES = 16 * 1024;

/** The request's JSON object body. A missing body reads as `{}`. */
export async function readJson(request: Request): Promise<Record<string, unknown>> {
  const text = await request.text();
  if (text.length > MAX_BODY_BYTES) {
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
