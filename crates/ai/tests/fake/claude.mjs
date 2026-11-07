// A fake `claude -p --input-format stream-json --output-format stream-json` for the tests in
// ../cli.rs: speaks just enough of the stream-json and control protocol (initialize,
// can_use_tool, set_model, interrupt, command_lifecycle) to drive BrowserMCP's Claude Code
// adapter without a real (paid) model.
//
// Started through a `claude.cmd` shim that sets FAKE_DIR (the test's temp folder):
// * every run appends `{argv, cwd}` to FAKE_DIR/runs.jsonl and every stdin line to
//   FAKE_DIR/stdin.jsonl;
// * FAKE_DIR/scenario.json (optional) changes behaviour: {"reject_resume": true} makes
//   `--resume=<id>` fail like a conversation Claude Code no longer has.
//
// Replies: a message containing "bash" asks to run the Bash tool (through `can_use_tool` when
// --permission-prompt-tool stdio is given, else refused like dontAsk mode); "withdraw" asks and
// cancels the request right away (`control_cancel_request`); "slow" works for up to 3 s: user
// messages arriving meanwhile are folded into that turn (lifecycle queued → started), an
// `interrupt` control request ends it early; "model?" answers with the current model
// (--model, then set_model). Everything is then echoed back as "echo: <text>" (folded
// messages joined with " + "). Messages with a `uuid` get `command_lifecycle` reports:
// `queued` on arrival, `started` when answered, `completed` after the turn.

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
const modelAt = argv.indexOf('--model');
let model = modelAt >= 0 ? argv[modelAt + 1] : 'default';

let inited = false;
let tools = 0;
let requests = 0;
let entries = 0;
/** request_id → resolve(response) for our can_use_tool requests. */
const waiting = new Map();
/** User messages are answered one after the other, like the real CLI. */
let queue = Promise.resolve();
/** The slow turn in progress: messages arriving now are folded into it. */
let current = null;

const uuid = () => `${sessionId}-e${++entries}`;
const lifecycle = (command_uuid, state) =>
  command_uuid && out({ type: 'system', subtype: 'command_lifecycle', command_uuid, state, uuid: uuid(), session_id: sessionId });
const say = (text) =>
  out({ type: 'assistant', uuid: uuid(), message: { role: 'assistant', content: [{ type: 'text', text }] }, session_id: sessionId });

function askPermission(name, input) {
  const request_id = `req-${++requests}`;
  out({
    type: 'control_request',
    request_id,
    request: { subtype: 'can_use_tool', tool_name: name, input, tool_use_id: `toolu_${tools}`, title: `Claude wants to run ${input.command}` },
  });
  return new Promise((resolve) => waiting.set(request_id, resolve));
}

async function answer(text, id) {
  lifecycle(id, 'started');
  if (!inited) {
    inited = true;
    out({ type: 'system', subtype: 'init', session_id: sessionId, cwd: process.cwd(), model, mcp_servers: [{ name: 'browsermcp', status: 'connected' }] });
  }
  const folded = [];
  if (text.includes('slow')) {
    say('working on it');
    current = { folded, interrupted: false, wake: null };
    const turn = current;
    await new Promise((resolve) => {
      turn.wake = resolve;
      setTimeout(resolve, 3000);
    });
    current = null;
    if (turn.interrupted) {
      out({ type: 'result', subtype: 'error_during_execution', is_error: true, result: 'interrupted', session_id: sessionId });
      for (const f of [{ id }, ...folded]) lifecycle(f.id, 'cancelled');
      return;
    }
  } else if (text.includes('withdraw')) {
    // Asks, then gives up at once (the cancel right behind the request, as one write).
    const request_id = `req-${++requests}`;
    process.stdout.write(
      JSON.stringify({ type: 'control_request', request_id, request: { subtype: 'can_use_tool', tool_name: 'Bash', input: { command: 'sleep 1' } } }) +
        '\n' +
        JSON.stringify({ type: 'control_cancel_request', request_id }) +
        '\n',
    );
    await new Promise((resolve) => waiting.set(request_id, resolve));
  } else if (text.includes('bash')) {
    const toolId = `toolu_${++tools}`;
    const input = { command: 'echo hi' };
    out({ type: 'assistant', uuid: uuid(), message: { role: 'assistant', content: [{ type: 'tool_use', id: toolId, name: 'Bash', input }] }, session_id: sessionId });
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
    out({ type: 'user', uuid: uuid(), message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: toolId, ...result }] }, session_id: sessionId });
  }
  if (text.includes('model?')) say(`model: ${model}`);
  say(`echo: ${[text, ...folded.map((f) => f.text)].join(' + ')}`);
  // Folded messages complete before the turn's result, the turn's own message after it.
  for (const f of folded) lifecycle(f.id, 'completed');
  out({ type: 'result', subtype: 'success', is_error: false, result: 'ok', uuid: `r-${entries}`, session_id: sessionId });
  lifecycle(id, 'completed');
}

const lines = createInterface({ input: process.stdin });
lines.on('line', (line) => {
  appendFileSync(join(dir, 'stdin.jsonl'), line + '\n');
  const msg = JSON.parse(line);
  if (msg.type === 'control_request') {
    const reply = () => out({ type: 'control_response', response: { subtype: 'success', request_id: msg.request_id, response: {} } });
    switch (msg.request.subtype) {
      case 'set_model':
        model = msg.request.model ?? 'default';
        return reply();
      case 'interrupt':
        if (current) {
          current.interrupted = true;
          current.wake();
        }
        return reply();
      default:
        return reply();
    }
  } else if (msg.type === 'control_response') {
    const resolve = waiting.get(msg.response.request_id);
    waiting.delete(msg.response.request_id);
    resolve?.(msg.response.response);
  } else if (msg.type === 'user') {
    const text = msg.message.content.map((c) => c.text).join('');
    lifecycle(msg.uuid, 'queued');
    if (current) {
      // Folded into the running turn, like the real CLI does with queued prompts.
      lifecycle(msg.uuid, 'started');
      current.folded.push({ text, id: msg.uuid });
      setTimeout(() => current?.wake(), 100);
      return;
    }
    queue = queue.then(() => answer(text, msg.uuid));
  }
});
// EOF: finish the current answer, unless it waits for a permission that can no longer come.
lines.on('close', () => (waiting.size ? process.exit(0) : queue.then(() => process.exit(0))));
