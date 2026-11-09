// The streams a component keeps open, so it closes them all when it goes. Pure, so it is
// tested without a window.

/** One stream of the registry, from the moment it is asked for. */
export interface StreamHandle {
  /** False once the stream ended or the registry closed it: its events are dropped. */
  live(): boolean;
  /** The stream opened: its closer is kept, or called at once when it is no longer live. */
  opened(close: () => void): void;
  /** The stream ended by itself: it is forgotten, and its later events are dropped. */
  ended(): void;
}

export interface StreamRegistry {
  /** Registers a stream before it opens, so one whose opening resolves late is closed too. */
  add(key: string): StreamHandle;
  /** Closes every live stream, and returns their keys. */
  closeAll(): string[];
  /** How many streams are live. */
  size(): number;
}

export function createStreamRegistry(): StreamRegistry {
  // A live stream's closer, null until it opened.
  const streams = new Map<symbol, { key: string; close: (() => void) | null }>();
  return {
    add(key) {
      const id = Symbol(key);
      streams.set(id, { key, close: null });
      return {
        live: () => streams.has(id),
        opened(close) {
          const stream = streams.get(id);
          if (stream) stream.close = close;
          else close();
        },
        ended() {
          streams.delete(id);
        },
      };
    },
    closeAll() {
      const keys = [...streams.values()].map(({ key, close }) => {
        close?.();
        return key;
      });
      streams.clear();
      return keys;
    },
    size: () => streams.size,
  };
}
