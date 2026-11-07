// The right-click menu of a web page, live. Headless tabs are off-screen, so the browser logs
// the menu it built ("context menu of an off-screen tab", menu.rs `run_offscreen`) and closes
// it; the test right-clicks through Playwright (CDP) and reads that log line.
//
// Our own pages (new tab, settings) and the chrome views cannot be checked here: headless
// tabs never show bmcp:// pages (backend.rs opens about:blank instead) and there is no chrome
// view. Their menus are covered by the unit tests in crates/cef/src/menu.rs.

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
  // The tab first: Playwright sees the pages that exist when it connects.
  tabs.push(await openTab(browser, fixture("login.html?menu=1")));
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

describe("context menu of a web page", () => {
  it("has navigation, Ask AI and Inspect on the page", async () => {
    const page = await pageAt("login.html?menu=1");
    const menu = await menuFor(page, "h1");
    for (const label of ["Back", "Reload", "Print…", "Ask AI about this page", "Inspect"]) expect(menu).toContain(`=${label}`);
    expect(menu).not.toContain("=Paste");
  });

  it("has the edit entries in a text field", async () => {
    const page = await pageAt("login.html?menu=1");
    const menu = await menuFor(page, "#email");
    for (const label of ["Undo", "Cut", "Copy", "Paste", "Select all"]) expect(menu).toContain(`=${label}`);
    expect(menu).not.toContain("=Back");
  });
});
