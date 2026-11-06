// The adblock gate in the network stack: the suite's browser starts with the user filter
// "/e2e-ads/" (data/filters/user.txt), so e2e/ads.html's banner script must never reach the
// fixture server, while its normal API call does.

import type { BrowserClient } from "@browsermcp/sdk";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { client, closeTabs, env, fixture, openTab, untilText } from "../lib/env.js";

let browser: BrowserClient;
const tabs: number[] = [];

async function hits(): Promise<Record<string, number>> {
  return (await fetch(`${env().fixtures}/hits`)).json();
}

beforeAll(async () => {
  browser = await client();
});
afterAll(async () => {
  await browser.adblock.toggle({ on: true, host: "localhost" }).catch(() => {});
  await closeTabs(browser, tabs);
  browser.close();
});

describe("adblock gate", () => {
  it("the user filter list is compiled into the engine", async () => {
    const stats = await browser.adblock.stats({});
    expect(stats.engine.loaded).toBe(true);
    expect(stats.engine.lists).toContain("user");
    const check = await browser.adblock.check({
      url: fixture("e2e-ads/banner.js"),
      source_url: fixture("e2e/ads.html"),
      type: "script",
    });
    expect(check.decision.action).toBe("block");
    expect(check.decision.reason).toBe("filter");
    expect(check.filter?.block).toBe(true);
    // Not an ad: allowed.
    const api = await browser.adblock.check({ url: fixture("api/price"), source_url: fixture("e2e/ads.html"), type: "fetch" });
    expect(api.decision.action).toBe("allow");
  });

  it("blocks the ad request, lets the rest through", async () => {
    const before = (await hits())["/e2e-ads/banner.js"] ?? 0;
    const tab = await openTab(browser, fixture("e2e/ads.html"));
    tabs.push(tab);
    await untilText(browser, tab, "ad blocked, price 10");
    expect((await hits())["/e2e-ads/banner.js"] ?? 0).toBe(before);
    const stats = await browser.adblock.stats({ tab });
    const mine = stats.tabs.find((t) => t.tab === tab);
    expect(mine?.blocked).toBeGreaterThanOrEqual(1);
    expect(mine?.by_filter).toBeGreaterThanOrEqual(1);
  });

  it("an allowlisted site gets its ads", async () => {
    await browser.adblock.toggle({ on: false, host: "localhost" });
    const tab = await openTab(browser, fixture("e2e/ads.html"));
    tabs.push(tab);
    await untilText(browser, tab, "ad loaded, price 10");
    await browser.adblock.toggle({ on: true, host: "localhost" });
  });
});
