// Voice dictation (crates/voice): the commands in the real browser, and the microphone rule —
// websites do not get it from the rule that lets our own bmcp://ui pages dictate. The harness
// has a fresh data folder (nothing downloaded) and no paid engine is ever called.

import type { BrowserClient } from "@browsermcp/sdk";
import { chromium, type Browser, type Page } from "playwright-core";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { client, closeTabs, env, fixture, openTab, until } from "../lib/env.js";

let browser: BrowserClient;
let pw: Browser | undefined;
const tabs: number[] = [];

beforeAll(async () => {
  browser = await client();
});
afterAll(async () => {
  await browser.call("voice.settings_set", { engine: "local", mode: "hold", hotkey: "Space" }).catch(() => {});
  await pw?.close().catch(() => {});
  await closeTabs(browser, tabs);
  browser.close();
});

/** The Playwright page of a tab whose URL contains `needle` (a fresh connection sees every tab). */
async function pageOf(needle: string): Promise<Page> {
  await pw?.close().catch(() => {});
  pw = await chromium.connectOverCDP(`http://127.0.0.1:${env().cdpPort}`);
  return until(`the tab ${needle} in Playwright`, async () =>
    pw!.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes(needle)),
  );
}

/** getUserMedia({audio}) in the page: "granted", the error name, or "pending" (a prompt). */
const askForMic = () =>
  Promise.race([
    navigator.mediaDevices.getUserMedia({ audio: true }).then(
      (s) => {
        s.getTracks().forEach((t) => t.stop());
        return "granted";
      },
      (e: DOMException) => e.name,
    ),
    new Promise<string>((r) => setTimeout(() => r("pending"), 4000)),
  ]);

describe("voice commands", () => {
  it("start with nothing downloaded and keep keys to themselves", async () => {
    const settings = await browser.call("voice.settings_get");
    expect(settings).toMatchObject({ enabled: true, engine: "local", mode: "hold", hotkey: "Space", auto_submit_tap: true, auto_submit_hold: false });
    const engines = await browser.call("voice.engines");
    expect(engines.engines.map((e) => e.id)).toEqual(["local", "openai", "groq", "elevenlabs", "deepgram", "custom"]);
    expect(engines.engines[0]).toMatchObject({ configured: false, offline: true, interim: true });
    const local = await browser.call("voice.local_status");
    expect(local.engine.installed).toBe(false);
    expect(local.models.some((m) => m.id === "large-v3-turbo-q5_0" && m.recommended)).toBe(true);
    expect(local.download).toBeNull();

    const saved = await browser.call("voice.settings_set", { deepgram: { api_key: "dg-e2e-secret-1234" }, hotkey: "ctrl+space" });
    expect(saved.hotkey).toBe("Ctrl+Space");
    expect(saved.deepgram.api_key_set).toBe(true);
    expect(JSON.stringify(saved)).not.toContain("dg-e2e-secret");
    await browser.call("voice.settings_set", { deepgram: { api_key: "" }, hotkey: "Space" });
  });

  it("explain what to set up instead of calling an engine", async () => {
    const wav = "UklGRiQAAABXQVZFZm10IBAAAAABAAEAgD4AAAB9AAACABAAZGF0YQAAAAA=";
    await expect(browser.call("voice.transcribe", { audio_base64: wav })).rejects.toThrow(/downloaded/);
    await expect(browser.call("voice.transcribe", { audio_base64: wav, engine: "openai" })).rejects.toThrow(/no API key/);
    await expect(browser.call("voice.stream_start", { engine: "groq" })).rejects.toThrow(/no API key/);
  });
});

describe("microphone permission", () => {
  // Our own pages (bmcp://ui) get it without a prompt: headless runs cannot show them (the
  // backend opens about:blank instead), so that half is covered by crates/cef scheme.rs tests.

  it("is not given to websites by that rule", async () => {
    const tab = await openTab(browser, fixture("form.html?mic=1"));
    tabs.push(tab);
    const page = await pageOf("form.html?mic=1");
    const result = await page.evaluate(askForMic);
    // Off-screen website tabs are denied; on screen the permission bar would ask. A machine
    // without any audio input (CI runners) answers NotFoundError before permissions are
    // asked: the site gets no microphone either way.
    expect(["NotAllowedError", "pending", "NotFoundError"]).toContain(result);
  });
});
