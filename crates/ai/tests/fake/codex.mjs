// A fake `codex app-server` for the tests in ../cli.rs: JSON-RPC lines over stdio (no
// "jsonrpc" field), enough to drive BrowserMCP's Codex adapter without a real model.
//
// Started through a `codex.cmd` shim that sets FAKE_DIR (the test's temp folder):
// * every run appends `{argv, cwd}` to FAKE_DIR/runs.jsonl, every line we receive to
//   FAKE_DIR/stdin.jsonl;
// * threads it created are remembered in FAKE_DIR/threads.json, so `thread/resume` works
//   across runs (and fails for unknown ids);
// * FAKE_DIR/scenario.json (optional): {"no_steer": true} answers `turn/steer` with "method
//   not found" (an older Codex), {"no_model_list": true} the same for `model/list`.
//
// A turn whose text contains "run" executes a shell command that needs approval
// (`item/commandExecution/requestApproval`); the decision decides the item's status. "slow"
// works for up to 3 s: `turn/steer` adds text to it, `turn/interrupt` ends it (status
// interrupted). "model?" answers with the model of the last `turn/start` that named one.
// Everything is echoed back as "echo: <text>" (steered text joined with " + ").

import { appendFileSync, existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createInterface } from 'node:readline';

const dir = process.env.FAKE_DIR;
appendFileSync(join(dir, 'runs.jsonl'), JSON.stringify({ argv: process.argv.slice(2), cwd: process.cwd() }) + '\n');
const scenario = existsSync(join(dir, 'scenario.json')) ? JSON.parse(readFileSync(join(dir, 'scenario.json'), 'utf8')) : {};
const threadsFile = join(dir, 'threads.json');
const threads = existsSync(threadsFile) ? JSON.parse(readFileSync(threadsFile, 'utf8')) : [];

const out = (v) => process.stdout.write(JSON.stringify(v) + '\n');
const notify = (method, params) => out({ method, params });
let turns = 0;
let serverRequests = 1000;
let model = 'default';
/** id of our request → resolve(result). */
const waiting = new Map();
/** The slow turn in progress. */
let current = null;

function request(method, params) {
  const id = ++serverRequests;
  out({ id, method, params });
  return new Promise((resolve) => waiting.set(id, resolve));
}

async function turn(threadId, turnId, text) {
  notify('turn/started', { threadId, turn: { id: turnId, status: 'inProgress', items: [] } });
  const steered = [];
  if (text.includes('slow')) {
    current = { turnId, steered, interrupted: false, wake: null };
    const running = current;
    await new Promise((resolve) => {
      running.wake = resolve;
      setTimeout(resolve, 3000);
    });
    current = null;
    if (running.interrupted) {
      notify('turn/completed', { threadId, turn: { id: turnId, status: 'interrupted', items: [] } });
      return;
    }
  }
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
  if (text.includes('model?')) {
    notify('item/completed', { threadId, turnId, item: { type: 'agentMessage', id: `q${turns}`, text: `model: ${model}` } });
  }
  const echo = [text, ...steered].join(' + ');
  notify('item/completed', { threadId, turnId, item: { type: 'agentMessage', id: `m${turns}`, text: `echo: ${echo}` } });
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
  const unknown = () => out({ id: msg.id, error: { code: -32601, message: `unknown ${msg.method}` } });
  const text = () => msg.params.input.map((i) => i.text).join('');
  switch (msg.method) {
    case 'initialize':
      return reply({ userAgent: 'fake-codex' });
    case 'initialized':
      return;
    case 'model/list':
      if (scenario.no_model_list) return unknown();
      return reply({
        data: [
          { id: 'fake-large', model: 'fake-large', displayName: 'Fake Large', description: 'the big one', isDefault: true },
          { id: 'fake-small', model: 'fake-small', displayName: 'Fake Small', description: 'fast' },
        ],
        nextCursor: null,
      });
    case 'thread/start': {
      const id = `th-${threads.length + 1}`;
      threads.push(id);
      writeFileSync(threadsFile, JSON.stringify(threads));
      return reply({ thread: { id } });
    }
    case 'thread/resume':
      if (!threads.includes(msg.params.threadId)) return out({ id: msg.id, error: { code: -32600, message: 'no rollout found' } });
      return reply({ thread: { id: msg.params.threadId } });
    case 'thread/rollback':
      if (!threads.includes(msg.params.threadId)) return out({ id: msg.id, error: { code: -32600, message: 'no such thread' } });
      return reply({ thread: { id: msg.params.threadId, turns: [] } });
    case 'turn/start': {
      if (msg.params.model) model = msg.params.model;
      const turnId = `u${++turns}`;
      reply({ turn: { id: turnId, status: 'inProgress', items: [] } });
      void turn(msg.params.threadId, turnId, text());
      return;
    }
    case 'turn/steer':
      if (scenario.no_steer) return unknown();
      if (!current || current.turnId !== msg.params.expectedTurnId) {
        return out({ id: msg.id, error: { code: -32600, message: 'no active turn to steer' } });
      }
      current.steered.push(text());
      reply({ turnId: current.turnId });
      setTimeout(() => current?.wake(), 100);
      return;
    case 'turn/interrupt':
      if (current) {
        current.interrupted = true;
        current.wake();
      }
      return reply({});
    default:
      return unknown();
  }
});
lines.on('close', () => process.exit(0));
