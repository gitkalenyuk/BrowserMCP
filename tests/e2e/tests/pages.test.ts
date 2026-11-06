// Agent-style page work: snapshot → act on refs, across iframes (same- and cross-site) and
// shadow DOM (open, nested, closed).

import type { BrowserClient } from "@browsermcp/sdk";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { client, closeTabs, env, fixture, openTab, refOf, untilText } from "../lib/env.js";

let browser: BrowserClient;
const tabs: number[] = [];

beforeAll(async () => {
  browser = await client();
});
afterAll(async () => {
  await closeTabs(browser, tabs);
  browser.close();
});

describe("snapshot → act", () => {
  it("logs in on login.html", async () => {
    const tab = await openTab(browser, fixture("login.html"));
    tabs.push(tab);
    const snap = await browser.page.snapshot({ tab });
    expect(snap.text).toContain('heading "Sign in"');
    await browser.page.act({ tab, ref: refOf(snap.text, 'textbox "Email"'), action: "fill", value: "mira@example.test" });
    await browser.page.act({ tab, ref: refOf(snap.text, 'textbox "Password"'), action: "fill", value: "hunter2" });
    await browser.page.act({ tab, ref: refOf(snap.text, 'checkbox "Remember me"'), action: "check" });
    const clicked = await browser.page.act({ tab, ref: refOf(snap.text, 'button "Sign in"'), action: "click" });
    expect(clicked).toBeDefined();
    await browser.nav.wait({ tab, for: "text", value: "Welcome mira@example.test" });
    const after = await browser.page.snapshot({ tab, mode: "full" });
    expect(after.text).toContain("Welcome mira@example.test");
  });

  it("a wrong password shows the alert", async () => {
    const tab = await openTab(browser, fixture("login.html"));
    tabs.push(tab);
    const snap = (await browser.page.snapshot({ tab })).text;
    await browser.page.act({ tab, ref: refOf(snap, 'textbox "Email"'), action: "fill", value: "x@example.test" });
    await browser.page.act({ tab, ref: refOf(snap, 'textbox "Password"'), action: "fill", value: "nope" });
    await browser.page.act({ tab, ref: refOf(snap, 'textbox "Password"'), action: "press", value: "Enter" });
    await untilText(browser, tab, "Wrong password");
  });

  it("stale refs are reported", async () => {
    const tab = await openTab(browser, fixture("login.html"));
    tabs.push(tab);
    const old = (await browser.page.snapshot({ tab })).text;
    await browser.nav.reload({ tab });
    await browser.nav.wait({ tab, for: "load" });
    await browser.page.snapshot({ tab });
    const err = await browser.page.act({ tab, ref: refOf(old, 'button "Sign in"'), action: "click" }).catch((e) => e);
    expect(err.code).toMatch(/STALE_REF|NOT_FOUND/);
  });
});

describe("frames and shadow DOM", () => {
  it("acts inside same-origin and cross-site iframes", async () => {
    const tab = await openTab(browser, fixture(`iframe.html?xport=${env().xport}`));
    tabs.push(tab);
    // The cross-site frame loads after the page's own load event.
    await untilText(browser, tab, "Top button");
    let snap = "";
    for (let i = 0; i < 40 && !snap.includes("Cross-origin button"); i++) {
      snap = (await browser.page.snapshot({ tab })).text;
      if (!snap.includes("Cross-origin button")) await new Promise((r) => setTimeout(r, 250));
    }
    expect(snap).toContain("Same-origin button");
    expect(snap).toContain("Cross-origin button");

    await browser.page.act({ tab, ref: refOf(snap, 'button "Top button"'), action: "click" });
    const notes = snap.split("\n").filter((l) => l.includes('textbox "Note"'));
    expect(notes.length).toBe(2);
    const noteRef = (line: string) => line.match(/@\d+:e\d+/)![0];
    await browser.page.act({ tab, ref: noteRef(notes[0]), action: "fill", value: "one" });
    await browser.page.act({ tab, ref: refOf(snap, 'button "Same-origin button"'), action: "click" });
    await browser.page.act({ tab, ref: noteRef(notes[1]), action: "fill", value: "two" });
    await browser.page.act({ tab, ref: refOf(snap, 'button "Cross-origin button"'), action: "click" });

    const full = (await browser.page.snapshot({ tab, mode: "full" })).text;
    expect(full).toContain("top clicked");
    expect(full).toContain("same clicked 1 note=one");
    expect(full).toContain("cross clicked 1 note=two");
  });

  it("acts inside open, nested and closed shadow roots", async () => {
    const tab = await openTab(browser, fixture("shadow.html"));
    tabs.push(tab);
    const snap = (await browser.page.snapshot({ tab })).text;
    await browser.page.act({ tab, ref: refOf(snap, 'textbox "Name"'), action: "fill", value: "Ada" });
    await browser.page.act({ tab, ref: refOf(snap, 'button "Save"'), action: "click" });
    await browser.page.act({ tab, ref: refOf(snap, 'button "Deep button"'), action: "click" });
    await browser.page.act({ tab, ref: refOf(snap, 'button "Closed button"'), action: "click" });
    const full = (await browser.page.snapshot({ tab, mode: "full" })).text;
    expect(full).toContain("saved Ada");
    expect(full).toContain("deep clicked");
    expect(full).toContain("closed clicked");
  });
});
