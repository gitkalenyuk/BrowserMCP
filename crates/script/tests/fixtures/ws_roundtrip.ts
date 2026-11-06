// Run by tests/ws_sink.rs against a fake JSON-RPC server that echoes
// `{ echo: method, params }`, or errors for "bad.cmd". Throws on any mismatch.
type Echo = { echo: string; params: unknown };

const r = (await browser.tabs.list({ all: true })) as Echo;
if (r.echo !== "tabs.list" || (r.params as { all: boolean }).all !== true) {
  throw new Error(`bad echo ${JSON.stringify(r)}`);
}

// Parallel calls: responses must be matched to requests by id.
const [a, b] = (await Promise.all([browser.a.one(), browser.b.two()])) as Echo[];
if (a.echo !== "a.one" || b.echo !== "b.two") {
  throw new Error(`mismatched ids: ${JSON.stringify([a, b])}`);
}

let failed = false;
try {
  await browser.bad.cmd();
} catch (e) {
  failed = (e as Error).message.includes("no such command");
}
if (!failed) throw new Error("expected bad.cmd to fail");
