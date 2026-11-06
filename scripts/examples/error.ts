/// <reference path="../../crates/script/types/browser.d.ts" />
// error.ts - shows what an uncaught error looks like.
//
//   cargo run -p bmcp-script -- run scripts/examples/error.ts
//
// Expected: exit code 1 and a stack trace that points at the TypeScript
// lines below (source maps), not at the transpiled JavaScript.

type Step = { name: string; selector: string };

function checkStep(step: Step): void {
  if (!step.selector) {
    throw new Error(`step "${step.name}" has no selector`);
  }
}

const tabs = await browser.tabs.list();
console.log(`got ${(tabs as unknown[]).length} tabs, now failing on purpose`);

checkStep({ name: "click login", selector: "" });
