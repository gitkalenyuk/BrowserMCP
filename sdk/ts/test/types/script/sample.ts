/// <reference path="../../../../../crates/script/types/browser.d.ts" />
// Type-level test of the generated script-host globals (browser, inputs, sleep, console).
// `tsc -p test/types/script/tsconfig.json` must pass; @ts-expect-error lines MUST be errors.

const { tabs } = await browser.tabs.list();
const first: BrowserMCP.TabsListOutput["tabs"][number] | undefined = tabs[0];
const page = await browser.nav.goto({ url: inputs.url ?? "https://example.com" });
const found = await browser.page.find({ role: "link" });
const viaCall = await browser.call("page.text", { limit: 100 });
await sleep(10);
console.log(first?.title, page.url, found.text, viaCall.truncated);
// Structured result for script.run / run rows (a runtime global, not a command).
browser.output({ title: first?.title, links: found.total });

// @ts-expect-error -- `url` is required.
await browser.nav.goto({});
// @ts-expect-error -- no such namespace.
await browser.nope.x();
// @ts-expect-error -- result fields are typed.
const n: number = page.url;
console.log(n);
