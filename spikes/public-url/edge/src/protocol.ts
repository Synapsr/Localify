// Wire protocol between the PublicEndpoint Durable Object and the Localify
// connector. Must stay in sync with connector/src/protocol.rs.
//
// Every WebSocket message is one binary frame:
//   byte 0      frame type
//   bytes 1..5  stream id, big-endian u32
//   bytes 5..   payload

export const FrameType = {
  RequestHead: 1,
  RequestBody: 2,
  RequestEnd: 3,
  ResponseHead: 4,
  ResponseBody: 5,
  ResponseEnd: 6,
  Reset: 7,
  WsText: 8,
  WsBinary: 9,
  WsClose: 10,
} as const;

export type FrameTypeValue = (typeof FrameType)[keyof typeof FrameType];

export interface Frame {
  type: number;
  streamId: number;
  payload: Uint8Array;
}

export interface RequestHead {
  method: string;
  path: string;
  headers: [string, string][];
  publicHost: string;
  label: string | null;
  hasBody: boolean;
  websocket: boolean;
}

export interface ResponseHead {
  status: number;
  headers: [string, string][];
}

export interface WsClose {
  code: number;
  reason: string;
}

/** Body chunks are split so one large stream cannot monopolize the socket. */
export const MAX_CHUNK_BYTES = 64 * 1024;

const HEADER_BYTES = 5;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

export function encodeFrame(
  type: FrameTypeValue,
  streamId: number,
  payload: Uint8Array = new Uint8Array(),
): ArrayBuffer {
  const bytes = new Uint8Array(HEADER_BYTES + payload.byteLength);
  bytes[0] = type;
  new DataView(bytes.buffer).setUint32(1, streamId);
  bytes.set(payload, HEADER_BYTES);
  return bytes.buffer;
}

export function decodeFrame(data: ArrayBuffer): Frame | null {
  if (data.byteLength < HEADER_BYTES) return null;
  const bytes = new Uint8Array(data);
  return {
    type: bytes[0],
    streamId: new DataView(data).getUint32(1),
    payload: bytes.subarray(HEADER_BYTES),
  };
}

export function encodeJson(value: unknown): Uint8Array {
  return encoder.encode(JSON.stringify(value));
}

export function decodeJson<T>(payload: Uint8Array): T {
  return JSON.parse(decoder.decode(payload)) as T;
}

export function encodeText(text: string): Uint8Array {
  return encoder.encode(text);
}

export function decodeText(payload: Uint8Array): string {
  return decoder.decode(payload);
}

export function* chunks(bytes: Uint8Array, size = MAX_CHUNK_BYTES): Generator<Uint8Array> {
  for (let offset = 0; offset < bytes.byteLength; offset += size) {
    yield bytes.subarray(offset, offset + size);
  }
}
