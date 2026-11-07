// A fake `claude -p --input-format stream-json --output-format stream-json` for the tests in
// ../cli.rs: speaks just enough of the stream-json and control protocol (initialize,
// can_use_tool) to drive BrowserMCP's Claude Code adapter without a real (paid) model.
//
// Started through a `claude.cmd` shim that sets FAKE_DIR (the test's temp folder):
// * every run appends `{argv, cwd}` to FAKE_DIR/runs.jsonl and every stdin line to
//   FAKE_DIR/stdin.jsonl;
// * FAKE_DIR/scenario.json (optional) changes behaviour: {"reject_resume": true} makes
//   `--resume=<id>` fail like a conversation Claude Code no longer has.
//
// Replies: a message containing "bash" asks to run the Bash tool (through `can_use_tool` when
// --permission-prompt-tool stdio is given, else refused like dontAsk mode); anything else is
// echoed back as "echo: <text>".

import { appendFileSync, existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const dir = process.env.FAKE_DIR;
const argv = process.argv.slice(2);
const scenario = existsSync(join(dir, 'scenario.json')) ? JSON.parse(readFileSync(join(dir, 'scenario.json'), 'utf8')) : {};
appendFileSync(join(dir, 'runs.jsonl'), JSON.stringify({ argv, cwd: process.cwd() }) + '\n');

const out = (v) => process.stdout.write(JSON.stringify(v) + '\n');
const resume = argv.find((a) => a.startsWith('--resume='))?.slice('--resume='.length);
if (resume && scenario.reject_resume) {
  process.stderr.write(`No conversation found with session ID: ${resume}\n`);
  process.exit(1);
}
const sessionId = resume ?? `sess-${Date.now()}`;
const prompts = argv.includes('--permission-prompt-tool');

let inited = false;
let tools = 0;
let requests = 0;
/** request_id → resolve(response) for our can_use_tool requests. */
const waiting = new Map();
/** User messages are answered one after the other, like the real CLI. */
let queue = Promise.resolve();

function askPermission(name, input) {
  const request_id = `req-${++requests}`;
  out({
    type: 'control_request',
    request_id,
    request: { subtype: 'can_use_tool', tool_name: name, input, tool_use_id: `toolu_${tools}`, title: `Claude wants to run ${input.command}` },
  });
  return new Promise((resolve) => waiting.set(request_id, resolve));
}

async function answer(text) {
  if (!inited) {
    inited = true;
    out({ type: 'system', subtype: 'init', session_id: sessionId, cwd: process.cwd(), mcp_servers: [{ name: 'browsermcp', status: 'connected' }] });
  }
  if (text.includes('bash')) {
    const id = `toolu_${++tools}`;
    const input = { command: 'echo hi' };
    out({ type: 'assistant', message: { role: 'assistant', content: [{ type: 'tool_use', id, name: 'Bash', input }] }, session_id: sessionId });
    let result;
    if (!prompts) {
      result = { content: 'Claude requested permissions to use Bash, but you haven\'t granted it yet.', is_error: true };
    } else {
      const response = await askPermission('Bash', input);
      result =
        response.behavior === 'allow'
          ? { content: `ran ${response.updatedInput.command}`, is_error: false }
          : { content: response.message, is_error: true };
    }
    out({ type: 'user', message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: id, ...result }] }, session_id: sessionId });
  }
  out({ type: 'assistant', message: { role: 'assistant', content: [{ type: 'text', text: `echo: ${text}` }] }, session_id: sessionId });
  out({ type: 'result', subtype: 'success', is_error: false, result: 'ok', session_id: sessionId });
}

const lines = createInterface({ input: process.stdin });
lines.on('line', (line) => {
  appendFileSync(join(dir, 'stdin.jsonl'), line + '\n');
  const msg = JSON.parse(line);
  if (msg.type === 'control_request' && msg.request.subtype === 'initialize') {
    out({ type: 'control_response', response: { subtype: 'success', request_id: msg.request_id, response: {} } });
  } else if (msg.type === 'control_response') {
    const resolve = waiting.get(msg.response.request_id);
    waiting.delete(msg.response.request_id);
    resolve?.(msg.response.response);
  } else if (msg.type === 'user') {
    const text = msg.message.content.map((c) => c.text).join('');
    queue = queue.then(() => answer(text));
  }
});
// EOF: finish the current answer, unless it waits for a permission that can no longer come.
lines.on('close', () => (waiting.size ? process.exit(0) : queue.then(() => process.exit(0))));
