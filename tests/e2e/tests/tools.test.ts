// Page tools on tools.html: PDF, file uploads (input and file chooser), JavaScript dialogs.

import { readFileSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { BrowserClient } from "@browsermcp/sdk";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { client, closeTabs, env, fixture, openTab, refOf, until, untilText } from "../lib/env.js";

let browser: BrowserClient;
let tab: number;
let snap: string;

beforeAll(async () => {
  browser = await client();
  tab = await openTab(browser, fixture("tools.html"));
  snap = (await browser.page.snapshot({ tab })).text;
});
afterAll(async () => {
  await closeTabs(browser, [tab]);
  browser.close();
});

describe("page tools", () => {
  it("page.pdf writes a PDF", async () => {
    const out = await browser.page.pdf({ tab, paper: "a4", print_background: true, path: "e2e-tools.pdf" });
    expect(out.path).toMatch(/e2e-tools\.pdf$/);
    expect(statSync(out.path).size).toBe(out.bytes);
    expect(out.bytes).toBeGreaterThan(2000);
    expect(readFileSync(out.path).subarray(0, 5).toString()).toBe("%PDF-");
  });

  it("page.upload fills a file input", async () => {
    const file = join(env().dataDir, "e2e-avatar.txt");
    writeFileSync(file, "hello upload");
    const out = await browser.page.upload({ tab, ref: refOf(snap, '"Avatar"'), files: [file] });
    expect(out.via).toBe("input");
    await untilText(browser, tab, "file: e2e-avatar.txt:12");
  });

  it("page.upload answers the file chooser a button opens", async () => {
    const a = join(env().dataDir, "e2e-photo-a.txt");
    const b = join(env().dataDir, "e2e-photo-b.txt");
    writeFileSync(a, "aaa");
    writeFileSync(b, "bbbbb");
    const out = await browser.page.upload({ tab, ref: refOf(snap, 'button "Choose photos"'), files: [a, b] });
    expect(out.via).toBe("chooser");
    await untilText(browser, tab, "chooser: e2e-photo-a.txt:3,e2e-photo-b.txt:5");
  });

  it("a confirm stays open for the agent, page.dialog answers it", async () => {
    const opened: string[] = [];
    const off = browser.on("page.dialog_opened", (e) => opened.push(`${e.type}: ${e.message}`));
    try {
      await browser.page.dialog_policy({ tab, confirm: "ask" });
      // The click itself either returns or reports DIALOG_OPEN (the page is frozen).
      const clicked = await browser.page
        .act({ tab, ref: refOf(snap, 'button "Show confirm"'), action: "click" })
        .catch((err) => err);
      if (clicked instanceof Error) expect((clicked as { code?: string }).code).toBe("DIALOG_OPEN");
      await until("page.dialog_opened", async () => opened.includes("confirm: Delete the file?"));
      const answered = await browser.page.dialog({ tab, action: "accept" });
      expect(answered.dialog?.type).toBe("confirm");
      await untilText(browser, tab, "confirm: true");
    } finally {
      off();
    }
  });

  it("a prompt is answered by policy", async () => {
    await browser.page.dialog_policy({ tab, prompt: "accept", prompt_text: "Grace" });
    await browser.page.act({ tab, ref: refOf(snap, 'button "Show prompt"'), action: "click" }).catch(() => {});
    await untilText(browser, tab, "prompt: Grace");
    await browser.page.dialog_policy({ tab, reset: true });
  });
});
