// LIVE end-to-end script (manual test, see crates/jobs/README.md "Live test").
// Signs in on tests/fixtures/sites/login.html with inputs.email / inputs.password and
// reports the welcome text with browser.output(). A wrong password throws, so the row fails.
//
// Each row opens its own tab: rows run in parallel and share the default profile.

const url = inputs.url ?? "http://127.0.0.1:8765/login.html";
const { tab } = (await browser.tabs.open({ url })) as { tab: number };
await browser.nav.wait({ tab, for: "load" });

type Found = { total: number; text: string };

/** The ref (`@S:eN`) of the first page.find match. */
async function find(args: Record<string, unknown>): Promise<string> {
  const found = (await browser.page.find({ tab, ...args })) as Found;
  const ref = found.text.match(/@\d+:e\d+/)?.[0];
  if (!ref) {
    // Same shape as a failed browser call, so the runner treats it as a broken selector.
    throw new Error(`browser.page.find failed: NOT_FOUND: nothing matches ${JSON.stringify(args)}`);
  }
  return ref;
}

await browser.page.act({ tab, ref: await find({ role: "textbox", name: "Email" }), action: "fill", value: inputs.email });
await browser.page.act({ tab, ref: await find({ role: "textbox", name: "Password" }), action: "fill", value: inputs.password });
await browser.page.act({ tab, ref: await find({ role: "button", name: "Sign in" }), action: "click" });

const welcome = (await browser.page.find({ tab, role: "heading", text: "Welcome" })) as Found;
if (welcome.total === 0) {
  const alert = (await browser.page.find({ tab, text: "Wrong password" })) as Found;
  throw new Error(alert.total > 0 ? "login failed: Wrong password" : "login failed: no welcome heading");
}
const text = welcome.text.match(/"([^"]+)"/)?.[1] ?? welcome.text;
console.log(`signed in as ${inputs.email}`);
browser.output({ welcome: text, tab });
