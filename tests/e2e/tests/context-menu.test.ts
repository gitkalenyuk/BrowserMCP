// The right-click menu: web pages get the full menu, our own pages (the new tab page) only
// editing / link / navigation entries. Headless tabs are off-screen, so the browser logs the
// menu it built ("context menu of an off-screen tab", menu.rs `run_offscreen`) and closes it;
// the test right-clicks through Playwright (CDP) and reads that log line.

import { readFileSync } from "node:fs";
import { join } from "node:path";

import type { BrowserClient } from "@browsermcp/sdk";
import { chromium, type Browser, type Page } from "playwright-core";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { client, closeTabs, env, fixture, openTab, until } from "../lib/env.js";

let browser: BrowserClient;
let pw: Browser | undefined;
const tabs: number[] = [];

beforeAll(async () => {
  browser = await client();
  pw = await chromium.connectOverCDP(`http://127.0.0.1:${env().cdpPort}`);
});
afterAll(async () => {
  await pw?.close().catch(() => {});
  await closeTabs(browser, tabs);
  browser.close();
});

/** Every "context menu of an off-screen tab" line the browser logged so far (its stderr,
 *  captured by the e2e launcher). */
function menuLines(): string[] {
  return readFileSync(join(env().dataDir, "logs", "e2e-browser.log"), "utf8")
    .split("\n")
    .filter((l) => l.includes("context menu of an off-screen tab"));
}

async function pageAt(urlPart: string): Promise<Page> {
  return until(`the tab ${urlPart} in Playwright`, async () =>
    pw!.contexts().flatMap((c) => c.pages()).find((p) => p.url().includes(urlPart)),
  );
}

/** Right-click `selector` and return the menu line the browser logged for it. */
async function menuFor(page: Page, selector: string): Promise<string> {
  const before = menuLines().length;
  await page.click(selector, { button: "right" });
  return until("the logged context menu", async () => menuLines()[before], 15_000);
}

describe("context menu", () => {
  it("a web page gets Ask AI and Inspect", async () => {
    tabs.push(await openTab(browser, fixture("form.html?menu=1")));
    const page = await pageAt("form.html?menu=1");
    const menu = await menuFor(page, "h1");
    expect(menu).toContain("Ask AI about this page");
    expect(menu).toContain("Inspect");
  });

  it("our new tab page gets copy & paste in its task box, no web entries", async () => {
    tabs.push(await openTab(browser, "bmcp://ui/newtab.html"));
    const page = await pageAt("bmcp://ui/newtab.html");
    await page.waitForSelector("textarea");
    const field = await menuFor(page, "textarea");
    for (const label of ["Undo", "Cut", "Copy", "Paste", "Select all"]) expect(field).toContain(`=${label}`);
    expect(field).not.toContain("Ask AI");
    expect(field).not.toContain("Inspect");
    // Bare page: Back / Forward / Reload only.
    const bare = await menuFor(page, "h1");
    expect(bare).toContain("=Reload");
    expect(bare).not.toContain("Print");
    expect(bare).not.toContain("Ask AI");
  });
});
