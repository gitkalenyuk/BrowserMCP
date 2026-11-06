// What `--headless` promises: no windows, off-screen tabs that render (screenshots, video),
// popups as tabs, and the default 1280x800 @1x page size.

import { spawnSync } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import type { BrowserClient } from "@browsermcp/sdk";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { client, closeTabs, env, fixture, openTab, refOf, until, untilText } from "../lib/env.js";

let browser: BrowserClient;
const tabs: number[] = [];

beforeAll(async () => {
  browser = await client();
});
afterAll(async () => {
  await closeTabs(browser, tabs);
  browser.close();
});

describe("headless", () => {
  it.runIf(process.platform === "win32")("has no window", () => {
    const out = spawnSync(
      "powershell",
      ["-NoProfile", "-NonInteractive", "-Command", `(Get-Process -Id ${env().pid}).MainWindowHandle`],
      { encoding: "utf8", windowsHide: true },
    );
    expect(out.status, out.stderr).toBe(0);
    expect(out.stdout.trim()).toBe("0");
  });

  it("pages are visible, 1280x800 at 1x", async () => {
    const tab = await openTab(browser, fixture("e2e/popup.html"));
    tabs.push(tab);
    await untilText(browser, tab, "1280x800@1 visible");
    const info = (await browser.tabs.list({})).tabs.find((t) => t.id === tab);
    expect(info?.url).toContain("e2e/popup.html");
  });

  it("screenshots render the page", async () => {
    const tab = await openTab(browser, fixture("login.html"));
    tabs.push(tab);
    const shot = await browser.page.screenshot({ tab, scale: 1 });
    expect(shot.mime_type).toBe("image/jpeg");
    expect([shot.width, shot.height]).toEqual([1280, 800]);
    const jpeg = Buffer.from(shot.data, "base64");
    expect(jpeg.subarray(0, 2).toString("hex")).toBe("ffd8");
    // A rendered page, not a blank frame: a white JPEG of this size is far smaller.
    expect(jpeg.length).toBeGreaterThan(8_000);
    const half = await browser.page.screenshot({ tab });
    expect(half.width).toBe(640);
  });

  it("popups and target=_blank links become tabs", async () => {
    const tab = await openTab(browser, fixture("e2e/popup.html"));
    tabs.push(tab);
    const snap = (await browser.page.snapshot({ tab })).text;
    await browser.page.act({ tab, ref: refOf(snap, 'button "Open popup"'), action: "click" });
    const popup = await until("window.open tab", async () =>
      (await browser.tabs.list({})).tabs.find((t) => t.url.includes("list.html?from=popup")),
    );
    tabs.push(popup.id);
    await browser.page.act({ tab, ref: refOf(snap, 'link "Open in new tab"'), action: "click" });
    const blank = await until("target=_blank tab", async () =>
      (await browser.tabs.list({})).tabs.find((t) => t.url.includes("form.html?from=blank")),
    );
    tabs.push(blank.id);
    // The popup is a working page of its own.
    await browser.nav.wait({ tab: blank.id, for: "load" });
    expect((await browser.page.snapshot({ tab: blank.id })).text).toContain("Country");
  });

  it.runIf(env().ffmpeg)("records a screencast video", async () => {
    const tab = await openTab(browser, fixture("tools.html"));
    tabs.push(tab);
    const started = await browser.record.video_start({ tab, fps: 10, format: "webm" });
    for (const dir of ["down", "down", "up"]) {
      await browser.page.act({ tab, action: "scroll", value: dir });
      await new Promise((r) => setTimeout(r, 500));
    }
    const video = await browser.record.video_stop({ id: started.id });
    expect(video.error ?? null).toBeNull();
    expect(video.frames_received).toBeGreaterThan(0);
    expect(video.bytes).toBeGreaterThan(1000);
    expect([video.width, video.height]).toEqual([1280, 800]);
    expect(existsSync(video.path) && statSync(video.path).size).toBe(video.bytes);
  });
});
