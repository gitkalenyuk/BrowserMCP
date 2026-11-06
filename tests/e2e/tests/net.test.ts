// Network tools on net.html (fetch, XHR, POST): capture + list, a mock rule, a response-stage
// breakpoint (body edits reach the page; status/header edits are refused, CEF drops them), and
// a request-stage breakpoint that holds a request until net.resume answers it (the server
// never sees it).

import type { BrowserClient } from "@browsermcp/sdk";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { client, closeTabs, fixture, openTab, refOf, until, untilText } from "../lib/env.js";

let browser: BrowserClient;
let tab: number;
let snap: string;

beforeAll(async () => {
  browser = await client();
  tab = await openTab(browser, fixture("list.html"));
  await browser.net.capture({ tab, on: true });
  await browser.nav.goto({ tab, url: fixture("net.html") });
  // The page's own startup calls end with the price.
  await untilText(browser, tab, "Price: 10");
  snap = (await browser.page.snapshot({ tab })).text;
});
afterAll(async () => {
  await browser.net.capture({ tab, on: false }).catch(() => {});
  await closeTabs(browser, [tab]);
  browser.close();
});

describe("net", () => {
  it("captures fetch, XHR and POST traffic", async () => {
    const api = await browser.net.list({ tab, filter: { url_contains: "/api/" } });
    expect(api.total).toBeGreaterThanOrEqual(5);
    expect(api.text).toContain("/api/items?page=1");
    expect(api.text).toContain("/api/items/42");
    const posts = await browser.net.list({ tab, filter: { method: "POST" } });
    expect(posts.text).toMatch(/POST\s+201/);
    // Only requests after the cursor.
    const later = await browser.net.list({ tab, cursor: api.next_cursor, filter: { url_contains: "/api/" } });
    expect(later.total).toBe(0);
  });

  it("a mock rule answers instead of the server", async () => {
    const rule = await browser.net.rule_add({
      tab,
      match: { url_glob: "*/api/price*" },
      action: { mock: { status: 200, body: '{"price":77}', headers: { "content-type": "application/json" } } },
    });
    try {
      await browser.page.act({ tab, ref: refOf(snap, 'button "Get price"'), action: "click" });
      await untilText(browser, tab, "Price: 77");
      const listed = await browser.net.list({ tab, filter: { url_contains: "/api/price" } });
      expect(listed.text).toContain(rule.id);
    } finally {
      await browser.net.rule_remove({ id: rule.id });
    }
  });

  it("a response-stage breakpoint: a new body reaches the page, a new status is refused", async () => {
    // The server answered {"price":10} with 200. CEF gives the page the server's status line
    // and headers after any response-stage answer (only a fulfill's body gets through; see
    // crates/net/src/intercept.rs), so net.resume refuses a status change instead of
    // reporting "fulfilled", keeps the request paused, and a body-only edit works.
    const bp = await browser.net.breakpoint({ tab, match: { url_glob: "*/api/price*" }, stage: "response" });
    const before = (await browser.page.text({ tab })).text.match(/fetch \/api\/price -> /g)?.length ?? 0;
    try {
      await browser.page.act({ tab, ref: refOf(snap, 'button "Get price"'), action: "click" });
      const paused = await until("a response-stage pause", async () => {
        const list = await browser.net.paused_list({ tab });
        return list.count > 0 ? list.text : null;
      });
      expect(paused).toMatch(/response GET 200 \S*\/api\/price/);
      const pauseId = paused.match(/^p\d+/)?.[0];
      expect(pauseId).toBeTruthy();

      // Status and headers: an honest error, nothing answered yet.
      await expect(
        browser.net.resume({
          pause_id: pauseId!,
          edits: { status: 203, response_body: '{"price":55}', headers: { "x-edited": "yes" } },
        }),
      ).rejects.toMatchObject({ code: "INVALID_PARAMS", message: expect.stringContaining("status 200 → 203") });
      expect((await browser.net.paused_list({ tab })).count).toBe(1);
      expect((await browser.page.text({ tab })).text.match(/fetch \/api\/price -> /g)?.length ?? 0).toBe(before);

      // The body alone: exactly what the page sees, with the server's 200.
      const resumed = await browser.net.resume({ pause_id: pauseId!, edits: { response_body: '{"price":55}' } });
      expect(resumed.done).toBe("fulfilled");
      const text = await untilText(browser, tab, "Price: 55");
      const logged = text.match(/fetch \/api\/price -> \d+/g) ?? [];
      expect(logged).toHaveLength(before + 1);
      expect(logged.at(-1)).toBe("fetch /api/price -> 200");
      expect((await browser.net.paused_list({ tab })).count).toBe(0);
    } finally {
      await browser.net.rule_remove({ id: bp.id });
    }
  });

  it("refuses response-stage rules that CEF would ignore", async () => {
    await expect(
      browser.net.rule_add({ tab, match: { url_glob: "*/api/price*", stage: "response" }, action: { set_headers: { "x-a": "1" } } }),
    ).rejects.toMatchObject({ code: "INVALID_PARAMS" });
    await expect(
      browser.net.rule_add({ tab, match: { url_glob: "*/api/price*", stage: "response" }, action: { mock: { status: 404 } } }),
    ).rejects.toMatchObject({ code: "INVALID_PARAMS" });
    expect((await browser.net.rule_list({ tab })).text).not.toContain("api/price");
  });

  it("a breakpoint holds a request until net.resume answers it", async () => {
    // Request stage: the request has not been sent; resuming with a status answers it
    // without sending, so the fixture server never sees it.
    const bp = await browser.net.breakpoint({ tab, match: { url_glob: "*/api/items?page=2*" }, stage: "request" });
    const hitsBefore = await (await fetch(fixture("hits"))).json();
    try {
      await browser.page.act({ tab, ref: refOf(snap, 'button "Load more"'), action: "click" });
      const paused = await until("a paused request", async () => {
        const list = await browser.net.paused_list({ tab });
        return list.count > 0 ? list.text : null;
      });
      expect(paused).toContain("/api/items?page=2");
      const pauseId = paused.match(/^p\d+/)?.[0];
      expect(pauseId).toBeTruthy();
      // Not answered yet: the page has not logged it.
      expect((await browser.page.text({ tab })).text).not.toContain("page=2 ->");
      const resumed = await browser.net.resume({
        pause_id: pauseId!,
        edits: { status: 202, response_body: '{"items":[],"page":2,"next":null}', headers: { "content-type": "application/json" } },
      });
      expect(resumed.text).toContain("fulfilled");
      await untilText(browser, tab, "fetch /api/items?page=2 -> 202");
      expect((await browser.net.paused_list({ tab })).count).toBe(0);
      const hitsAfter = await (await fetch(fixture("hits"))).json();
      expect(hitsAfter["/api/items"]).toBe(hitsBefore["/api/items"]);
    } finally {
      await browser.net.rule_remove({ id: bp.id });
    }
  });
});
