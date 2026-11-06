# MVP check: Claude Code drives BrowserMCP by refs only (2026-11-06)

Setup: `browsermcp.exe --disable-features=CalculateNativeWinOcclusion --disable-backgrounding-occluded-windows`,
fixture sites from `tests/fixtures/sites/` on `python -m http.server 47920`, Claude Code 2.1.317
headless with `.mcp.json` (`claude -p ... --strict-mcp-config --allowedTools "mcp__browsermcp__*"`).

Prompt: get past the cookie banner on cookie.html and press "Get started"; log in on login.html with a
wrong password first, then ann@example.test / hunter2; never use screenshots.

Result (Claude's own report):

1. Cookie page: a "We use cookies" dialog covered the page; clicking "Accept" removed it.
2. "Get started" added the status "Started!".
3. Wrong password → alert "Wrong password", form stayed.
4. Right password → heading "Welcome ann@example.test", form and error gone.

Tools: `page_act` ×7 (3 fill, 4 click), `nav_goto` ×2, `page_snapshot` ×1, `page_find` ×1. No screenshots.

Follow-ups found: Claude Code warned `unknown format "uint64"` for schemars number formats → stripped
from all command schemas. Occluded windows drop synthetic clicks unless the two switches above are
set → being made default in crates/cef (Phase 2).
