// Type-level test of the generated SDK types: `tsc -p test/types/tsconfig.json` must pass.
// Lines marked @ts-expect-error MUST be errors (tsc fails if they compile).

import { BrowserError, connect, type Commands, type PageFindOutput } from "@browsermcp/sdk";

const browser = await connect({ url: "ws://127.0.0.1:47800/v1/ws", timeout: 30_000 });

// Arguments and results are typed per command.
const { tab } = await browser.tabs.open({ url: "https://example.com" });
const page = await browser.nav.goto({ url: "https://example.com", tab, timeout_secs: 20 }, { timeout: 25_000 });
const title: string = page.title;
const snap = await browser.page.snapshot({ tab, mode: "interactive" });
const snapId: number = snap.snap;
const found: PageFindOutput = await browser.page.find({ tab, role: "link" });
await browser.page.act({ tab, ref: "@1:e1", action: "click", returns: null });
await browser.nav.wait({ for: "text", value: "Example" });
const tabs = (await browser.tabs.list()).tabs.map((t) => `${t.id} ${t.url} ${t.loading}`);

// The low-level call is typed by command name.
const ping = await browser.call("system.ping");
const version: string = ping.version;
const goto: Commands["nav.goto"]["output"] = await browser.call("nav.goto", { url: "https://example.com" });

// Events: exact topics get typed payloads; patterns get `unknown`.
const off = browser.on("command.finished", (event, topic) => {
  const ms: number = event.duration_ms;
  const ok: boolean = event.ok;
  console.log(topic, ms, ok, event.caller.kind);
});
browser.on("tab.*", (data, topic) => console.log(topic, data));
off();
await browser.events.subscribe({ topics: ["command.*"] });

try {
  await browser.page.act({ ref: "@9:e9", action: "fill", value: "x" });
} catch (err) {
  if (err instanceof BrowserError && err.retry === "after_snapshot") console.log(err.code, err.hint, err.candidates);
}

// @ts-expect-error -- `url` is required.
await browser.nav.goto({});
// @ts-expect-error -- unknown argument (typo of `tab`).
await browser.page.snapshot({ tab_id: 1 });
// @ts-expect-error -- not one of the actions.
await browser.page.act({ ref: "@1:e1", action: "tap" });
// @ts-expect-error -- no such command.
await browser.call("tabs.lst");
// @ts-expect-error -- results are typed: `title` is a string.
const wrong: number = page.title;
// @ts-expect-error -- `tabs` has no `nope` command.
await browser.tabs.nope({ tab: 1 });

console.log(title, snapId, found.total, tabs, version, goto.url, wrong);
browser.close();
