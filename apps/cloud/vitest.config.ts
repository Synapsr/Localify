/// <reference types="node" />
import { join } from "node:path";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    // The tests run in Node, where workerd's own modules do not exist.
    alias: { "cloudflare:workers": join(import.meta.dirname, "test", "cloudflare-workers.ts") },
  },
  test: {
    include: ["test/**/*.test.ts"],
  },
});
