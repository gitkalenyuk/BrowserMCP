/// <reference path="../../../../crates/script/types/browser.d.ts" />
// hello-plugin: the smallest useful BrowserMCP plugin.
//
// Install it into a running browser:
//   curl -X POST http://127.0.0.1:47800/v1/cmd/plugins.install -d '{"path":"scripts/examples/plugins/hello-plugin"}'
// then read what it logged:
//   curl -X POST http://127.0.0.1:47800/v1/cmd/plugins.logs -d '{"name":"hello-plugin"}'
//
// A plugin is a script that keeps running: `browser.on(...)` handlers keep it alive. It may
// only call the commands listed under "permissions" in plugin.json and only receives the
// events listed under "events" (plus `plugin.command` for its own palette entries).

const me = inputs.plugin ?? "hello-plugin";
console.log(`${me} started`);

interface TabUpdated {
  id: number;
  url: string;
  title: string;
  loading: boolean;
}

// Every finished load (and title change) of every tab.
browser.on("tab.updated", (tab: TabUpdated) => {
  if (!tab.loading) console.log(`tab ${tab.id} loaded: ${tab.title} <${tab.url}>`);
});

// Ctrl+K entries from plugin.json ("ui.palette") arrive as `plugin.command {plugin, id}`.
browser.on("plugin.command", async (cmd: { plugin: string; id: string }) => {
  if (cmd.plugin !== me || cmd.id !== "count-tabs") return;
  const { tabs } = await browser.tabs.list({});
  console.log(`${tabs.length} tab(s) open`);
  browser.output({ tabs: tabs.length });
});
