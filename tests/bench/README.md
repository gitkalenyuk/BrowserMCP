# MVP benchmark

The Phase 3 exit criterion: a CLI agent solves 10 tasks on local fixture sites through
BrowserMCP's MCP server, without screenshots.

```sh
cargo xtask bench --agent claude-code --tasks all
cargo xtask bench --agent codex --tasks table,scroll --timeout 240
```

Options: `--model <m>`, `--timeout <secs per task>` (300), `--port <api port>` (47841),
`--no-build`, `--keep-open`.

The runner (`xtask/src/bench.rs`) builds and starts its own browser
(`--api-port 47841 --data-dir target/live-data`), serves `tests/fixtures/sites/` on a free
port, and runs every task headless:

* Claude Code: `claude -p <prompt> --output-format stream-json --verbose --strict-mcp-config
  --mcp-config <file with http://127.0.0.1:47841/mcp?agent=bench> --allowedTools
  "mcp__browsermcp__*"` (`page_screenshot` disallowed);
* Codex: `codex exec --json -c mcp_servers.browsermcp.url="…?agent=bench" -`.

Before each task the `bench` agent is forgotten (`agents.forget`) and one blank tab is left.
The fixture pages (`tests/fixtures/sites/bench/`) report what the agent did to the fixture
server (`bench.js` → `POST /bench/result`); after the run each task's check
(`xtask/src/bench/tasks.rs`) looks at those reports and at `tabs.list` over REST.

| id | exercises | success condition |
|---|---|---|
| search | search results page | only the KeyCo K7 keyboard added to the cart |
| wizard | 3-step form wizard | Dana Reed / dana@example.test / Pro / yearly / terms accepted |
| pagination | pagination list | price of SKU Q-4417 (page 4) submitted: $63.10 |
| compare | multi-tab compare | Borealis 15 chosen, both spec pages open in tabs |
| table | table extraction | sum of Shipped orders submitted: 566.00 |
| modal | modal dialog | "Old Site" deleted via the dialog, the others kept |
| select | native selects + custom listbox | Ukraine / express / 3 / Forest green / gift wrap |
| scroll | infinite scroll | post #42 liked, no other post |
| login | login form (no iframe) | signed in as mira, statement downloaded |
| recovery | error recovery | message sent after the simulated 503 (≥ 2 attempts) |

Results: `results/<date>.json` (one entry per agent: pass/fail, reason, tool calls per tool,
tokens incl. cache, cost, time) and `results/<date>.md` (the summary table).
