/// <reference path="../../crates/script/types/browser.d.ts" />
// hello.ts - smallest useful BrowserMCP script.
//
// Run it (mock browser, no real browser needed):
//   cargo run -p bmcp-script -- run scripts/examples/hello.ts --input name=Ada --input times=3
//
// `browser` and `inputs` are globals provided by bmcp-script-host.
// Every `browser.<namespace>.<method>(args)` call becomes ONE JSON-RPC style
// command "<namespace>.<method>" sent to the browser.

interface Tab {
  id: number;
  url: string;
  title: string;
}

const name: string = inputs.name ?? "world";
const times: number = Number(inputs.times ?? "2");

console.log(`hello, ${name}`);

// Top-level await works: the file is an ES module.
const { tabs } = (await browser.tabs.list()) as { tabs: Tab[] };
console.log(`found ${tabs.length} tabs`);
for (const tab of tabs) {
  console.log(`- ${tab.id}: ${tab.title} <${tab.url}>`);
}

// A loop over browser commands, the typical "run it N times" shape.
for (let i = 1; i <= times; i++) {
  const res = await browser.nav.goto({ url: `https://example.com/?page=${i}` });
  console.log(`goto #${i} -> ${JSON.stringify(res)}`);
}

const page = await browser.page.text();
console.log(`page.text -> ${JSON.stringify(page)}`);
console.log("done");
