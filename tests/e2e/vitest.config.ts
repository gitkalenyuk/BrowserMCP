import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const src = (path: string) => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  resolve: {
    // The SDK and protocol from source: nothing has to be built besides the browser.
    alias: {
      "@browsermcp/sdk": src("../../sdk/ts/src/index.ts"),
      "@browsermcp/protocol": src("../../sdk/protocol/src/index.ts"),
    },
  },
  test: {
    include: ["tests/**/*.test.ts"],
    // Starts ONE `browsermcp --headless` + the fixture server for the whole suite.
    globalSetup: ["setup/global.ts"],
    // Files share that browser (adblock, dialog policies and recordings are browser-wide),
    // so they run one after another; tests inside a file too.
    fileParallelism: false,
    testTimeout: 60_000,
    hookTimeout: 120_000,
    reporters: ["default"],
  },
});
