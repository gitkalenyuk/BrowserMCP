// Recording → generated TypeScript → script host, and a mass run (5 CSV rows, 3 at once, each
// row in its own isolated profile). Running scripts needs bmcp-script-host next to the
// browser (`cargo build -p bmcp-script`, or `cargo xtask e2e --all`); without it those tests
// are skipped.

import { writeFileSync } from "node:fs";
import { join } from "node:path";
import type { BrowserClient } from "@browsermcp/sdk";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { client, closeTabs, env, fixture, openTab, refOf, until, untilText } from "../lib/env.js";

let browser: BrowserClient;
const tabs: number[] = [];
const host = env().scriptHost;

beforeAll(async () => {
  browser = await client();
});
afterAll(async () => {
  await closeTabs(browser, tabs);
  browser.close();
});

describe("record → codegen", () => {
  let recordingTab = 0;
  let scriptName = "";

  it("records agent actions and generates a script", async () => {
    const tab = await openTab(browser, fixture("login.html"));
    tabs.push(tab);
    recordingTab = tab;
    const rec = await browser.record.start({ tab, name: "e2e-login" });
    const snap = (await browser.page.snapshot({ tab })).text;
    await browser.page.act({ tab, ref: refOf(snap, 'textbox "Email"'), action: "fill", value: "rec@example.test" });
    await browser.page.act({ tab, ref: refOf(snap, 'textbox "Password"'), action: "fill", value: "hunter2" });
    await browser.page.act({ tab, ref: refOf(snap, 'button "Sign in"'), action: "click" });
    await untilText(browser, tab, "Welcome rec@example.test");
    const stopped = await browser.record.stop({ id: rec.id });
    expect(stopped.steps).toBeGreaterThanOrEqual(3);

    const gen = await browser.record.codegen({ id: rec.id, name: "e2e-login-recorded", parametrize: true });
    expect(gen.path).toMatch(/[\\/]scripts[\\/]e2e-login-recorded\.ts$/);
    scriptName = "e2e-login-recorded";
    expect(gen.code).toContain('"Email"');
    expect(gen.code).toContain('"Sign in"');
    // The password is never written into the script.
    expect(gen.code).not.toContain("hunter2");
    expect(gen.inputs.find((i) => i.secret)?.name).toBeTruthy();
  });

  it.runIf(host)("the generated script runs in bmcp-script-host", async () => {
    expect(scriptName).not.toBe("");
    // Leave the login page: the script navigates (the most recent tab) back to it itself.
    await browser.nav.goto({ tab: recordingTab, url: fixture("list.html") });
    const recordings = await browser.record.list({});
    expect(JSON.stringify(recordings)).toContain("e2e-login");
    const secret = (await browser.script.get({ name: scriptName })).code.match(/need\("(\w+)"\)/)?.[1] ?? "password";
    const out = await browser.script.run({ name: scriptName, inputs: { [secret]: "hunter2", email: "gen@example.test" } });
    expect(out.ok, `${out.stderr}\n${out.stdout}`).toBe(true);
    const texts = await Promise.all(
      (await browser.tabs.list({})).tabs.map(async (t) => (await browser.page.text({ tab: t.id })).text),
    );
    expect(texts.some((t) => /Welcome (gen|rec)@example\.test/.test(t))).toBe(true);
  });
});

const ROW_SCRIPT = `
const { id: tab } = await browser.tabs.open({ url: inputs.url });
await browser.nav.wait({ tab, for: "load" });
async function ref(role: string, name: string): Promise<string> {
  const found = await browser.page.find({ tab, role, name });
  const r = found.text.match(/@\\d+:e\\d+/)?.[0];
  if (!r) throw new Error("no " + role + " " + name);
  return r;
}
await browser.page.act({ tab, ref: await ref("textbox", "Email"), action: "fill", value: inputs.email });
await browser.page.act({ tab, ref: await ref("textbox", "Password"), action: "fill", value: inputs.password });
await browser.page.act({ tab, ref: await ref("button", "Sign in"), action: "click" });
let text = "";
for (let i = 0; i < 40; i++) {
  text = (await browser.page.text({ tab })).text;
  if (text.includes("Welcome") || text.includes("Wrong password")) break;
  await sleep(100);
}
const outcome = text.includes("Welcome " + inputs.email) ? "welcome" : text.includes("Wrong password") ? "wrong" : "unknown";
browser.output({ email: inputs.email, outcome });
await browser.tabs.close({ tab });
`;

describe("mass run", () => {
  it.runIf(host)("5 CSV rows, parallel 3, isolated profiles: every row correct", async () => {
    await browser.script.save({ name: "e2e_login_row", code: ROW_SCRIPT });
    const rows = [
      ["ann@example.test", "hunter2", "welcome"],
      ["bob@example.test", "wrong", "wrong"],
      ["cyd@example.test", "hunter2", "welcome"],
      ["dee@example.test", "hunter2", "welcome"],
      ["eve@example.test", "letmein", "wrong"],
    ];
    const csv = join(env().dataDir, "e2e-rows.csv");
    writeFileSync(
      csv,
      ["url,email,password,expect", ...rows.map(([email, password, expect]) => `${fixture("login.html")},${email},${password},${expect}`)].join("\n") + "\n",
    );
    const run = await browser.run.start({
      script: "e2e_login_row",
      data: { csv_path: csv },
      parallel: 3,
      profile: "isolated",
      heal: "off",
      retries: 0,
      timeout_secs: 60,
    });
    expect(run.total).toBe(5);
    expect(run.options.parallel_requested).toBe(3);
    expect(run.options.profile).toBe("isolated");

    const done = await until(
      "the run to finish",
      async () => {
        const status = await browser.run.status({ id: run.id });
        return ["finished", "failed", "cancelled"].includes(status.status) ? status : null;
      },
      150_000,
    );
    const results = await browser.run.results({ id: run.id });
    const summary = results.rows.map((r) => `${r.row} ${r.status} ${JSON.stringify(r.result)} ${r.error?.message ?? ""}`);
    expect(done.status, summary.join("\n")).toBe("finished");
    expect(done.counts.ok, summary.join("\n")).toBe(5);
    for (const row of results.rows) {
      const result = row.result as { email: string; outcome: string };
      expect(result.email).toBe(row.inputs.email);
      expect(result.outcome, `row ${row.row}`).toBe(row.inputs.expect);
    }
  }, 180_000);
});
