// ==UserScript==
// @name        Example banner
// @description Puts a banner on top of example.com after every page load.
// @match       https://example.com/*
// @match       https://*.example.com/*
// @run-at      document-end
// ==/UserScript==
/// <reference path="../../../crates/script/types/browser.d.ts" />
//
// Install it into a running browser (data/userscripts/example-banner.ts):
//   curl -X POST http://127.0.0.1:47800/v1/cmd/userscripts.save \
//        -d "{\"name\":\"example-banner\",\"code\":$(jq -Rs . < scripts/examples/userscripts/example-banner.ts)}"
// then open https://example.com/. The browser runs this file once per matching page load
// with `inputs.tab` (the tab id) and `inputs.url`.

const tab = Number(inputs.tab);

// The function body runs inside the page (isolated world: same DOM, own globals).
const { value } = await browser.userscripts.inject({
  tab,
  js: `
    if (document.getElementById("bmcp-banner")) return "already there";
    const banner = document.createElement("div");
    banner.id = "bmcp-banner";
    banner.textContent = "Hello from a BrowserMCP userscript";
    banner.style.cssText =
      "position:fixed;top:0;left:0;right:0;z-index:2147483647;padding:8px;" +
      "background:#5b4cff;color:#fff;font:14px system-ui;text-align:center";
    document.body.prepend(banner);
    return "added";
  `,
});
console.log(`banner on tab ${tab} (${inputs.url}): ${value}`);
