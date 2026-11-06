import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    // Test against the protocol sources, so `pnpm test` works without building it first.
    alias: {
      "@browsermcp/protocol": fileURLToPath(new URL("../protocol/src/index.ts", import.meta.url)),
    },
  },
  test: {
    include: ["test/**/*.test.ts"],
    // The tests wait for conditions, not fixed times; these limits only catch a real hang.
    // vitest's defaults (5 s per test, 10 s per hook) fail on a busy machine, e.g. the first
    // fetch() in a worker loading undici while cargo builds run in parallel.
    testTimeout: 60_000,
    hookTimeout: 60_000,
  },
});
