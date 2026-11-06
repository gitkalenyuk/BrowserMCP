// Helpers for test files: the shared browser's endpoints, an SDK client, tabs on fixtures.

import { connect, type BrowserClient } from "@browsermcp/sdk";
import { inject } from "vitest";

import { sleep } from "./launch.js";

export function env() {
  return inject("e2e");
}

export const restUrl = () => `http://127.0.0.1:${env().apiPort}/v1`;
export const wsUrl = () => `ws://127.0.0.1:${env().apiPort}/v1/ws`;
export const mcpUrl = () => `http://127.0.0.1:${env().apiPort}/mcp`;

/** URL of a page in tests/fixtures/sites/. */
export function fixture(path: string): string {
  return `${env().fixtures}/${path.replace(/^\//, "")}`;
}

/** The typed SDK client over WebSocket (events included). Close it in afterAll. */
export function client(): Promise<BrowserClient> {
  return connect({ url: wsUrl(), reconnect: false });
}

/** Open `url` in a new (off-screen, since headless) tab and wait for it to load. */
export async function openTab(browser: BrowserClient, url: string): Promise<number> {
  const { id } = await browser.tabs.open({ url });
  await browser.nav.wait({ tab: id, for: "load", timeout_secs: 30 });
  return id;
}

/** Close tabs, ignoring ones that are gone already. */
export async function closeTabs(browser: BrowserClient, tabs: number[]): Promise<void> {
  for (const tab of tabs) {
    await browser.tabs.close({ tab }).catch(() => {});
  }
}

/** The page's visible text. */
export async function pageText(browser: BrowserClient, tab: number): Promise<string> {
  return (await browser.page.text({ tab })).text;
}

/** Wait until the page's text contains `needle` (case-sensitive); returns the text. */
export async function untilText(browser: BrowserClient, tab: number, needle: string, ms = 15_000): Promise<string> {
  let last = "";
  try {
    return await until(`"${needle}" in tab ${tab}`, async () => {
      last = await pageText(browser, tab);
      return last.includes(needle) ? last : null;
    }, ms);
  } catch (err) {
    throw new Error(`${(err as Error).message}\n--- page text ---\n${last.slice(-1500)}`);
  }
}

/** Poll `check` until it returns a truthy value (returned) or `ms` pass (throws). */
export async function until<T>(what: string, check: () => Promise<T | undefined | null | false>, ms = 15_000): Promise<T> {
  const deadline = Date.now() + ms;
  let last: unknown;
  for (;;) {
    try {
      const value = await check();
      if (value) return value;
    } catch (err) {
      last = err;
    }
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}${last ? `: ${last}` : ""}`);
    await sleep(250);
  }
}

/** Ref of the first snapshot line containing `needle`, e.g. `textbox "Email"` → `@3:e2`. */
export function refOf(snapshot: string, needle: string): string {
  const line = snapshot.split("\n").find((l) => l.includes(needle));
  const ref = line?.match(/@\d+:e\d+/)?.[0];
  if (!ref) throw new Error(`no ref for ${needle} in snapshot:\n${snapshot}`);
  return ref;
}
