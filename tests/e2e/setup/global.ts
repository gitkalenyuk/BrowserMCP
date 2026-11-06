// Global setup: ONE headless browser + the fixture server for the whole suite. Builds
// nothing — it expects target/debug/browsermcp.exe (`cargo xtask e2e` builds, then runs this).
//
// The browser gets its own temporary data folder, free API and CDP ports, and an adblock
// user filter list ("/e2e-ads/") so the adblock test has something to block without
// downloading filter lists. Teardown quits the browser with `system.quit` (and kills its PID
// if that does not work) and kills the fixture server's PID.

import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { TestProject } from "vitest/node";

import {
  browserExe,
  freePort,
  hasFfmpeg,
  removeDir,
  scriptHostExe,
  startBrowser,
  startFixtures,
  type BrowserProcess,
  type FixtureServer,
} from "../lib/launch.js";

export interface E2eEnv {
  exe: string;
  apiPort: number;
  cdpPort: number;
  pid: number;
  dataDir: string;
  /** http://localhost:<port> — tests/fixtures/sites/ */
  fixtures: string;
  /** Cross-site port for iframe.html?xport= */
  xport: number;
  /** bmcp-script-host was built (scripts, runs and generated code can run). */
  scriptHost: boolean;
  ffmpeg: boolean;
}

declare module "vitest" {
  export interface ProvidedContext {
    e2e: E2eEnv;
  }
}

let browser: BrowserProcess | undefined;
let fixtures: FixtureServer | undefined;
let dataDir: string | undefined;

export default async function setup(project: TestProject) {
  const exe = browserExe();
  dataDir = mkdtempSync(join(tmpdir(), "bmcp-e2e-"));
  mkdirSync(join(dataDir, "filters"), { recursive: true });
  writeFileSync(join(dataDir, "filters", "user.txt"), "! e2e: the fixture's fake ads\n/e2e-ads/\n");

  fixtures = await startFixtures();
  const [apiPort, cdpPort] = [await freePort(), await freePort()];
  try {
    browser = await startBrowser({ exe, dataDir, apiPort, args: ["--cdp-port", String(cdpPort)] });
  } catch (err) {
    fixtures.stop();
    throw err;
  }
  const env: E2eEnv = {
    exe,
    apiPort,
    cdpPort,
    pid: browser.pid,
    dataDir,
    fixtures: fixtures.base,
    xport: fixtures.xport,
    scriptHost: scriptHostExe(exe) !== null,
    ffmpeg: hasFfmpeg(),
  };
  console.log(
    `[e2e] browser pid ${env.pid} api ${apiPort} cdp ${cdpPort}, fixtures ${env.fixtures} (pid ${fixtures.pid}), ` +
      `data ${dataDir}, script host: ${env.scriptHost ? "yes" : "no"}, ffmpeg: ${env.ffmpeg ? "yes" : "no"}`,
  );
  project.provide("e2e", env);

  return async () => {
    const quit = await browser?.stop();
    if (quit === false) console.warn(`[e2e] the browser did not quit on system.quit; killed pid ${browser?.pid}`);
    fixtures?.stop();
    if (dataDir && !process.env.BMCP_E2E_KEEP) {
      await removeDir(dataDir);
    } else if (dataDir) {
      console.log(`[e2e] kept ${dataDir}`);
    }
  };
}
