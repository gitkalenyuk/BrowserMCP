// A fake `codex app-server` for the tests in ../cli.rs: JSON-RPC lines over stdio (no
// "jsonrpc" field), enough to drive BrowserMCP's Codex adapter without a real model.
//
// Started through a `codex.cmd` shim that sets FAKE_DIR (the test's temp folder):
// * every run appends `{argv, cwd}` to FAKE_DIR/runs.jsonl, every line we receive to
//   FAKE_DIR/stdin.jsonl;
// * threads it created are remembered in FAKE_DIR/threads.json, so `thread/resume` works
//   across runs (and fails for unknown ids).
//
// A turn whose text contains "run" executes a shell command that needs approval
// (`item/commandExecution/requestApproval`); the decision decides the item's status.

import { appendFileSync, existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const dir = process.env.FAKE_DIR;
appendFileSync(join(dir, 'runs.jsonl'), JSON.stringify({ argv: process.argv.slice(2), cwd: process.cwd() }) + '\n');
const threadsFile = join(dir, 'threads.json');
const threads = existsSync(threadsFile) ? JSON.parse(readFileSync(threadsFile, 'utf8')) : [];

const out = (v) => process.stdout.write(JSON.stringify(v) + '\n');
const notify = (method, params) => out({ method, params });
let turns = 0;
let serverRequests = 1000;
/** id of our request → resolve(result). */
const waiting = new Map();

function request(method, params) {
  const id = ++serverRequests;
  out({ id, method, params });
  return new Promise((resolve) => waiting.set(id, resolve));
}

async function turn(threadId, text) {
  const turnId = `u${++turns}`;
  notify('turn/started', { threadId, turn: { id: turnId, status: 'inProgress', items: [] } });
  if (text.includes('run')) {
    const item = { type: 'commandExecution', id: `c${turns}`, command: 'rm -rf build', cwd: process.cwd(), status: 'inProgress' };
    notify('item/started', { threadId, turnId, item });
    const { decision } = await request('item/commandExecution/requestApproval', {
      threadId,
      turnId,
      itemId: item.id,
      startedAtMs: Date.now(),
      command: item.command,
      cwd: item.cwd,
      reason: 'clean the build folder',
    });
    const accepted = decision === 'accept' || decision === 'acceptForSession';
    notify('item/completed', {
      threadId,
      turnId,
      item: { ...item, status: accepted ? 'completed' : 'declined', exitCode: accepted ? 0 : null, aggregatedOutput: accepted ? 'removed' : null },
    });
  }
  notify('item/completed', { threadId, turnId, item: { type: 'agentMessage', id: `m${turns}`, text: `echo: ${text}` } });
  notify('turn/completed', { threadId, turn: { id: turnId, status: 'completed', items: [] } });
}

const lines = createInterface({ input: process.stdin });
lines.on('line', (line) => {
  appendFileSync(join(dir, 'stdin.jsonl'), line + '\n');
  const msg = JSON.parse(line);
  if (msg.method === undefined) {
    // An answer to one of our requests.
    const resolve = waiting.get(msg.id);
    waiting.delete(msg.id);
    resolve?.(msg.result ?? {});
    return;
  }
  const reply = (result) => out({ id: msg.id, result });
  switch (msg.method) {
    case 'initialize':
      return reply({ userAgent: 'fake-codex' });
    case 'initialized':
      return;
    case 'thread/start': {
      const id = `th-${threads.length + 1}`;
      threads.push(id);
      writeFileSync(threadsFile, JSON.stringify(threads));
      return reply({ thread: { id } });
    }
    case 'thread/resume':
      if (!threads.includes(msg.params.threadId)) return out({ id: msg.id, error: { code: -32600, message: 'no rollout found' } });
      return reply({ thread: { id: msg.params.threadId } });
    case 'turn/start':
      reply({ turn: { id: `u${turns + 1}` } });
      void turn(msg.params.threadId, msg.params.input.map((i) => i.text).join(''));
      return;
    case 'turn/interrupt':
      return reply({});
    default:
      return out({ id: msg.id, error: { code: -32601, message: `unknown ${msg.method}` } });
  }
});
lines.on('close', () => process.exit(0));
