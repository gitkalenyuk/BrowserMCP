// Website languages (browser.languages_set, crates/languages): the list reaches pages as
// navigator.languages and as the Accept-Language header, applied live (no restart), and the
// browser's own locale is only offered from the shipped locale files.

import type { BrowserClient } from "@browsermcp/sdk";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { client, closeTabs, fixture, openTab, untilText } from "../lib/env.js";

let browser: BrowserClient;
const tabs: number[] = [];
/** The list before the test, put back afterwards (other test files share the browser). */
let before: { web: string[]; web_automatic: boolean } | undefined;

beforeAll(async () => {
  browser = await client();
  before = await browser.call("browser.languages_get");
});
afterAll(async () => {
  if (before) await browser.call("browser.languages_set", { web: before.web_automatic ? [] : before.web }).catch(() => {});
  await closeTabs(browser, tabs);
  browser.close();
});

describe("website languages", () => {
  it("reaches navigator.languages and Accept-Language at once", async () => {
    const set = await browser.call("browser.languages_set", { web: ["de", "UK", "en"] });
    expect(set.web).toEqual(["de", "uk", "en"]);
    expect(set.web_automatic).toBe(false);

    const tab = await openTab(browser, fixture("e2e/languages.html"));
    tabs.push(tab);
    const text = await untilText(browser, tab, " done");
    expect(text).toContain("languages=de,uk,en");
    // Chromium adds the q-values itself.
    expect(text).toContain("header=de,uk;q=0.9,en;q=0.8");

    // A change applies to the next load without a restart.
    await browser.call("browser.languages_set", { web: ["pl", "en"] });
    const second = await openTab(browser, fixture("e2e/languages.html"));
    tabs.push(second);
    const after = await untilText(browser, second, " done");
    expect(after).toContain("languages=pl,en");
    expect(after).toContain("header=pl,en;q=0.9");
  });

  it("refuses what is not a language and keeps the list", async () => {
    await expect(browser.call("browser.languages_set", { web: ["de;q=0.5"] })).rejects.toThrow(/language tag/);
    const now = await browser.call("browser.languages_get");
    expect(now.web).toEqual(["pl", "en"]);
  });

  it("offers only the shipped Chromium locales", async () => {
    const info = await browser.call("browser.languages_get");
    expect(info.browser_locales).toContain("en-US");
    await expect(browser.call("browser.languages_set", { browser_locale: "xx-YY" })).rejects.toThrow(/not shipped/);
    expect(info.restart_required).toBe(false);
  });
});
