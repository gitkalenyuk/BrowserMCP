// A headless browser is an automation host: it outlives its last tab and ends with
// `system.quit`. Each test here starts a browser of its own (own data folder and port).

import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { connect } from "@browsermcp/sdk";
import { afterEach, describe, expect, it } from "vitest";

import { env, fixture, untilText } from "../lib/env.js";
import { exited, freePort, healthy, killPid, removeDir, rest, sleep, startBrowser, waitExit } from "../lib/launch.js";

const cleanups: Array<() => void | Promise<void>> = [];
afterEach(async () => {
  for (const cleanup of cleanups.splice(0)) await cleanup();
});

function tempData(): string {
  const dir = mkdtempSync(join(tmpdir(), "bmcp-e2e-life-"));
  cleanups.push(() => removeDir(dir));
  return dir;
}

describe("headless lifecycle", () => {
  it("--window-size / --device-scale, survives its last tab, quits on system.quit", async () => {
    const apiPort = await freePort();
    const proc = await startBrowser({
      exe: env().exe,
      dataDir: tempData(),
      apiPort,
      args: ["--window-size", "1024x640", "--device-scale", "2"],
    });
    cleanups.unshift(() => killPid(proc.pid)); // no-op once it quit
    const browser = await connect({ url: `ws://127.0.0.1:${apiPort}/v1/ws`, reconnect: false });
    try {
      const { id } = await browser.tabs.open({ url: fixture("e2e/popup.html") });
      await untilText(browser, id, "1024x640@2 visible");
      const shot = await browser.page.screenshot({ tab: id, scale: 1 });
      expect(shot.width).toBeGreaterThanOrEqual(1024);

      // Close every tab (the start page too): the process stays.
      for (const tab of (await browser.tabs.list({})).tabs) await browser.tabs.close({ tab: tab.id });
      await sleep(2000);
      expect((await browser.tabs.list({})).tabs).toEqual([]);
      expect(exited(proc.pid)).toBe(false);
      expect(await healthy(apiPort)).toBe(true);

      // ...and still opens tabs.
      const again = await browser.tabs.open({ url: fixture("list.html") });
      await browser.nav.wait({ tab: again.id, for: "load" });
      await browser.tabs.close({ tab: again.id });
    } finally {
      browser.close();
    }

    expect(await rest(apiPort, "system.quit")).toEqual({ quitting: true });
    expect(await waitExit(proc.pid, 30_000)).toBe(true);
    expect(await proc.exitCode).toBe(0);
  });

  it("mcp-stdio --headless starts a headless browser", async () => {
    const apiPort = await freePort();
    const dataDir = tempData();
    const stdio = spawn(env().exe, ["mcp-stdio", "--headless", "--data-dir", dataDir, "--api-port", String(apiPort)], {
      stdio: ["pipe", "pipe", "ignore"],
      windowsHide: true,
    });
    let browserPid = 0;
    cleanups.unshift(() => {
      if (browserPid) killPid(browserPid);
      killPid(stdio.pid!);
    });
    const firstLine = new Promise<string>((resolve, reject) => {
      let out = "";
      stdio.stdout!.on("data", (chunk: Buffer) => {
        out += chunk.toString();
        if (out.includes("\n")) resolve(out.split("\n")[0]);
      });
      stdio.once("exit", (code) => reject(new Error(`mcp-stdio exited (${code})`)));
    });
    stdio.stdin!.write(
      JSON.stringify({
        jsonrpc: "2.0",
        id: 1,
        method: "initialize",
        params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "e2e", version: "0" } },
      }) + "\n",
    );
    const reply = JSON.parse(await firstLine);
    expect(reply.result.serverInfo).toBeDefined();

    const endpoint = JSON.parse(readFileSync(join(dataDir, "runtime", "endpoint.json"), "utf8"));
    browserPid = endpoint.pid;
    const tabs = await rest(apiPort, "tabs.list");
    expect(tabs.tabs.length).toBe(1);

    stdio.stdin!.end();
    await rest(apiPort, "system.quit");
    expect(await waitExit(browserPid, 30_000)).toBe(true);
  });
});
