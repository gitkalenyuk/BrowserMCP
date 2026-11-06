// Open example.com, snapshot it, find its link. Needs a running browser and `pnpm -r build`.
//
//   node sdk/ts/examples/example-com.mjs [ws://127.0.0.1:47800/v1/ws]

import { BrowserError, connect } from "@browsermcp/sdk";

const browser = await connect({ url: process.argv[2] ?? "ws://127.0.0.1:47800/v1/ws" });
browser.on("command.finished", (e) => console.log(`  [event] ${e.name} ${e.duration_ms} ms ok=${e.ok}`));

const ping = await browser.system.ping();
console.log(`connected to ${ping.name} ${ping.version}`);

const { tab } = await browser.tabs.open({ url: "https://example.com" });
await browser.nav.wait({ tab, for: "load" });
console.log(`tab ${tab}:`, (await browser.tabs.list()).tabs.find((t) => t.id === tab));

const snap = await browser.page.snapshot({ tab });
console.log(`\npage_snapshot (snap ${snap.snap}):\n${snap.text}`);

const found = await browser.page.find({ tab, role: "link" });
console.log(`\npage_find role=link (${found.total} total):\n${found.text}`);

try {
  await browser.page.act({ tab, ref: "@999:e1", action: "click" });
} catch (err) {
  if (!(err instanceof BrowserError)) throw err;
  console.log(`\nexpected error: ${err.code} retry=${err.retry} hint=${err.hint ?? "-"}`);
}

browser.close();
