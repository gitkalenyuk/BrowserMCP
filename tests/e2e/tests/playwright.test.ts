// External automation through the CDP port (`--cdp-port`): Playwright's connectOverCDP
// drives an off-screen tab that BrowserMCP opened. playwright-core only — no browser download.

import type { BrowserClient } from "@browsermcp/sdk";
import { chromium, type Browser } from "playwright-core";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { client, closeTabs, env, fixture, openTab, until } from "../lib/env.js";

let browser: BrowserClient;
let pw: Browser | undefined;
const tabs: number[] = [];

beforeAll(async () => {
  browser = await client();
});
afterAll(async () => {
  await pw?.close().catch(() => {}); // disconnects; the browser keeps running
  await closeTabs(browser, tabs);
  browser.close();
});

describe("Playwright connectOverCDP", () => {
  it("drives a headless tab", async () => {
    const tab = await openTab(browser, fixture("form.html?pw=1"));
    tabs.push(tab);
    pw = await chromium.connectOverCDP(`http://127.0.0.1:${env().cdpPort}`);
    const page = await until("the tab in Playwright", async () =>
      pw!.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes("form.html?pw=1")),
    );
    expect(await page.title()).toContain("Preferences");
    expect(await page.evaluate(() => [innerWidth, innerHeight, document.visibilityState])).toEqual([1280, 800, "visible"]);
    await page.selectOption("#country", "pl");
    await page.check("#news");
    await page.click("button[type=submit]");
    await page.waitForSelector("#saved:has-text('Saved: country=pl news=yes')");
    // The same page, seen through BrowserMCP.
    const text = (await browser.page.text({ tab })).text;
    expect(text).toContain("Saved: country=pl news=yes");
  });
});
