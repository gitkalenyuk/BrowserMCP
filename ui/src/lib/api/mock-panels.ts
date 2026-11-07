// Fakes for the side panels' commands (ai.*, term.*, net.*), used by MockApi so `pnpm dev`
// demos every panel without a backend. Output shapes and text formats follow the Rust
// commands (crates/ai, crates/net); behaviour is only plausible, not a spec.

import { autoTitle } from '../ai/sessions';
import { RpcError } from './client';
import { createProviderMocks } from './mock-ai-providers';
import type {
  AiAgentAccess,
  AiPermission,
  AiPermissionDecision,
  AiSession,
  AiTranscriptEntry,
  ApiEvents,
  EventName,
  Method,
  NetAction,
  NetEdits,
  NetListFilter,
  NetMatch,
  NetPaused,
  NetStage,
  Params,
  Result,
  TabId,
  TermInfo,
} from './types';

type PanelMethod = Extract<Method, `ai.${string}` | `term.${string}` | `net.${string}`>;
export type PanelHandlers = { [M in PanelMethod]: (params: Params<M>) => Result<M> | Promise<Result<M>> };
type Emit = <E extends EventName>(event: E, payload: ApiEvents[E]) => void;

const AGENT_COLORS = ['#3b82f6', '#f97316', '#22c55e', '#a855f7', '#ec4899', '#14b8a6', '#eab308', '#ef4444'];

// ---- net text formats (crates/net/src/format.rs) -----------------------------------------

function fmtSize(bytes: number): string {
  if (bytes < 1000) return `${bytes}B`;
  if (bytes < 1_000_000) return `${(bytes / 1000).toFixed(1)}kB`;
  return `${(bytes / 1_000_000).toFixed(1)}MB`;
}

function fmtDuration(ms: number): string {
  return ms < 1000 ? `${Math.round(ms)}ms` : `${(ms / 1000).toFixed(1)}s`;
}

function shortUrl(url: string, max = 120): string {
  const chars = [...url];
  if (chars.length <= max) return url;
  const tail = Math.floor((max * 2) / 5);
  const head = max - tail - 1;
  return `${chars.slice(0, head).join('')}…${chars.slice(chars.length - tail).join('')}`;
}

interface Frame {
  sent: boolean;
  opcode: number;
  data: string;
  ts: number;
}

interface Entry {
  id: number;
  method: string;
  url: string;
  type: string;
  mime: string;
  status: number | null;
  statusText: string;
  error: string | null;
  size: number;
  start: number;
  end: number | null;
  fromCache: boolean;
  reqHeaders: [string, string][];
  resHeaders: [string, string][];
  reqBody: string | null;
  resBody: string | null;
  frames: Frame[];
  notes: string[];
}

function line(e: Entry): string {
  const status = e.error ? 'ERR' : e.status !== null ? String(e.status) : '…';
  const size = e.fromCache ? 'cache' : e.size > 0 ? fmtSize(e.size) : '-';
  const time = e.end !== null ? fmtDuration(e.end - e.start) : e.type === 'websocket' && !e.error ? 'open' : '-';
  let out = `#${e.id} ${e.method} ${status} ${e.type} ${size} ${time} ${shortUrl(e.url)}`;
  if (e.error) out += ` !${e.error}`;
  if (e.type === 'websocket' && e.frames.length) out += ` (${e.frames.length} frames)`;
  if (e.notes.length) out += ` [${e.notes.join('; ')}]`;
  return out;
}

function filterMatches(f: NetListFilter | undefined, e: Entry): boolean {
  if (!f) return true;
  if (f.url_contains && !e.url.toLowerCase().includes(f.url_contains.toLowerCase())) return false;
  if (f.method && e.method.toLowerCase() !== f.method.toLowerCase()) return false;
  if (f.type && !f.type.split(',').some((t) => t.trim().toLowerCase() === e.type)) return false;
  if (f.status !== undefined) {
    const s = String(f.status).toLowerCase();
    if (s === 'failed') return e.error !== null;
    if (s === 'pending') return e.status === null && !e.error;
    if (e.status === null || e.error) return false;
    if (s.endsWith('xx')) return String(e.status)[0] === s[0];
    return String(e.status) === s;
  }
  return true;
}

function globRegex(glob: string): RegExp {
  let re = '^';
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === '*') re += '.*';
    else if (c === '?') re += '.';
    else if (c === '\\' && i + 1 < glob.length) re += glob[++i].replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    else re += c.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }
  return new RegExp(`${re}$`);
}

interface MockRule {
  id: string;
  tab: TabId;
  stage: NetStage;
  regex: RegExp;
  method: string | null;
  describe: string;
  action: NetAction | 'pause';
  hits: number;
}

function actionShort(a: NetAction | 'pause'): string {
  if (a === 'block' || a === 'pause') return a;
  if ('redirect' in a) return `redirect → ${a.redirect.url}`;
  if ('set_headers' in a) return `set_headers ${Object.keys(a.set_headers).join(',')}`;
  if ('remove_headers' in a) return `remove_headers ${a.remove_headers.join(',')}`;
  if ('rewrite_body' in a) return `rewrite_body ${JSON.stringify(a.rewrite_body.find)} → ${JSON.stringify(a.rewrite_body.replace)}`;
  return `mock ${a.mock.status ?? 200}`;
}

function ruleLine(r: MockRule): string {
  return `${r.id} tab ${r.tab} ${r.stage} ${r.describe} → ${actionShort(r.action)} (${r.hits} hits)`;
}

/** Like `intercept::resume_problem`: why these edits cannot reach the page from a response-stage
 *  pause. A server response keeps its status and headers under CEF; a failed one has no body. */
export function responseProblem(p: NetPaused, edits: NetEdits): string | null {
  const norm = (h: [string, string][]) =>
    h
      .map(([n, v]) => `${n.toLowerCase()}\n${v}`)
      .sort()
      .join('\n\n');
  const original: [string, string][] = (p.response_headers ?? []).map((h) => [h.name, h.value]);
  let edited = original;
  for (const [name, value] of Object.entries(edits.headers ?? {})) {
    edited = edited.filter(([n]) => n.toLowerCase() !== name.toLowerCase());
    if (value !== null) edited = [...edited, [name, value]];
  }
  const headersChanged = norm(original) !== norm(edited);
  if (p.error) {
    if ((edits.status !== undefined || headersChanged) && edits.response_body === undefined) {
      return `the response failed (${p.error}), so it has no body to keep: give response_body together with status or headers`;
    }
    return null;
  }
  const code = p.status ?? 200;
  const newStatus = edits.status !== undefined && edits.status !== code ? edits.status : null;
  if (newStatus === null && !headersChanged) return null;
  const what =
    newStatus !== null && headersChanged ? `status ${code} → ${newStatus} and the headers` : newStatus !== null ? `status ${code} → ${newStatus}` : 'the headers';
  return `cannot change ${what} at the response stage: the server already answered and the browser (CEF) gives the page the server's status and headers whatever the edit says; only response_body can be replaced here`;
}

interface MockPause {
  info: NetPaused;
  entry: Entry;
  deadline: number;
  timer: ReturnType<typeof setTimeout>;
  /** Continue the request; null = unchanged (timeout, breakpoint removed). */
  done: (edits: NetEdits | null) => void;
}

interface TabTraffic {
  entries: Entry[];
  capture: boolean;
  timer: ReturnType<typeof setInterval> | null;
}

/** Traffic a page might make: [method, url, type, mime, status, size, ms, body?]. */
const PAGE_LOAD: [string, string, string, string, number, number, number, string?][] = [
  ['GET', 'https://shop.example.com/', 'document', 'text/html', 200, 48210, 182, '<!doctype html>\n<html lang="en">\n<head><title>Example Shop</title></head>\n<body>…</body>\n</html>'],
  ['GET', 'https://shop.example.com/assets/app-4f2a91.js', 'script', 'application/javascript', 200, 312400, 96],
  ['GET', 'https://shop.example.com/assets/app-77c0de.css', 'stylesheet', 'text/css', 200, 38120, 41],
  ['GET', 'https://fonts.example-cdn.com/inter-var.woff2', 'font', 'font/woff2', 200, 98340, 63],
  ['GET', 'https://shop.example.com/img/hero@2x.webp', 'image', 'image/webp', 200, 184002, 120],
  ['GET', 'https://shop.example.com/img/logo.svg', 'image', 'image/svg+xml', 304, 0, 12],
  ['GET', 'https://ads.tracker.example.net/pixel.gif?uid=8f3a', 'image', 'image/gif', 200, 43, 210],
  ['GET', 'https://api.shop.example.com/v2/session', 'fetch', 'application/json', 200, 512, 74, '{"user":{"id":81,"name":"Ada"},"cart":3}'],
  ['GET', 'https://api.shop.example.com/v2/products?category=laptops&page=1', 'xhr', 'application/json', 200, 18342, 233, '{"items":[{"id":"x1-carbon","price":1499},{"id":"t14s","price":1129}],"page":1,"pages":12}'],
  ['POST', 'https://api.shop.example.com/v2/events', 'fetch', 'text/plain', 204, 0, 38],
  ['GET', 'https://api.shop.example.com/v2/recommendations', 'xhr', 'application/json', 500, 88, 812, '{"error":"upstream timeout"}'],
];

const LIVE: [string, string, string, string, number, number, number, string?][] = [
  ['GET', 'https://api.shop.example.com/v2/products?category=laptops&page=2', 'xhr', 'application/json', 200, 17120, 190, '{"items":[{"id":"zbook","price":1890}],"page":2,"pages":12}'],
  ['GET', 'https://api.shop.example.com/v2/cart', 'fetch', 'application/json', 200, 734, 66, '{"items":3,"total":3757}'],
  ['POST', 'https://api.shop.example.com/v2/cart/items', 'fetch', 'application/json', 201, 211, 140, '{"ok":true}'],
  ['GET', 'https://api.shop.example.com/v2/stock/x1-carbon', 'xhr', 'application/json', 404, 61, 52, '{"error":"not found"}'],
  ['POST', 'https://api.shop.example.com/v2/events', 'fetch', 'text/plain', 204, 0, 31],
  ['GET', 'https://shop.example.com/img/products/x1-carbon.webp', 'image', 'image/webp', 200, 64210, 88],
];

/** Commands of the fake shell. */
function shellPrompt(shell: string): string {
  if (shell.startsWith('cmd')) return 'C:\\Users\\you>';
  if (shell.startsWith('bash')) return '\x1b[32myou@mock\x1b[0m:\x1b[34m~\x1b[0m$ ';
  return 'PS C:\\Users\\you> ';
}

interface MockTerm {
  info: TermInfo;
  line: string;
}

export function createPanelMocks(emit: Emit, activeTab: () => TabId | null): PanelHandlers {
  // ---- AI --------------------------------------------------------------------------------
  // Providers (built-in and the user's own) live in mock-ai-providers.ts.
  const providerMocks = createProviderMocks();
  const getProvider = providerMocks.get;
  const sessions = new Map<string, AiSession>();
  const transcripts = new Map<string, AiTranscriptEntry[]>();
  const runs = new Map<string, ReturnType<typeof setTimeout>[]>();
  /** Messages sent while a chat works, oldest first (taken when the turn ends, like Codex). */
  const queues = new Map<string, string[]>();
  let sessionCount = 0;

  const session = (id: string): AiSession => {
    const s = sessions.get(id);
    if (!s) throw new RpcError('NOT_FOUND', `no AI session '${id}'`);
    return s;
  };

  /** The `ai.session` event with the SessionInfo fields (title, model, archived, queued…). */
  const publish = (s: AiSession) => {
    const { id, ...info } = s;
    emit('ai.session', { session: id, ...info });
  };

  const setStatus = (s: AiSession, status: AiSession['status']) => {
    if (s.status === status) return;
    s.status = status;
    s.updated_ms = Date.now();
    publish(s);
  };

  const userCount = (id: string) => (transcripts.get(id) ?? []).filter((e) => e.type === 'user').length;

  /** The automatic title: the first user message, until then the provider's name. */
  const retitle = (s: AiSession) => {
    if (s.title_set) return;
    const first = transcripts.get(s.id)?.find((e) => e.type === 'user');
    s.title = first && 'text' in first ? autoTitle(first.text) : getProvider(s.provider).name;
  };

  // Two closed chats, so the History drawer has something to show in `pnpm dev`.
  const seedClosed = (id: string, provider: string, model: string | null, title: string, ago: number, entries: AiTranscriptEntry[]) => {
    const created = Date.now() - ago;
    sessions.set(id, {
      id,
      provider,
      model,
      color: AGENT_COLORS[(sessions.size + 3) % AGENT_COLORS.length],
      status: 'stopped',
      tabs: [],
      created_ms: created,
      title,
      title_set: false,
      archived: true,
      updated_ms: created + 60_000,
      queued: 0,
      access: null,
      pending_permissions: 0,
    });
    transcripts.set(id, entries.map((e) => ({ ...e, ts_ms: created })));
  };
  seedClosed('claude-code-90', 'claude-code', 'sonnet', 'Find direct flights to Lviv next Friday', 26 * 3600_000, [
    { type: 'user', text: 'Find direct flights to Lviv next Friday' },
    { type: 'tool_call', name: 'tabs_open', args: { url: 'https://flights.example.com/?to=LWO' } },
    { type: 'tool_result', name: 'tabs_open', ok: true, summary: '{"tab":7}' },
    {
      type: 'assistant',
      text: '## Direct flights\n\n| Time | Airline | Price |\n|---|---|--:|\n| 07:40 | LOT | €129 |\n| 18:15 | Wizz Air | €84 |\n\nThe **evening Wizz Air** flight is the cheapest. Book at [flights.example.com](https://flights.example.com).',
    },
    { type: 'done', reason: 'end_turn' },
  ]);
  seedClosed('codex-91', 'codex', null, 'Clean up the build folder', 4 * 86400_000, [
    { type: 'user', text: 'Clean up the build folder' },
    { type: 'assistant', text: 'Removed `build/` (14 files):\n\n```powershell\nRemove-Item -Recurse build\n```' },
    { type: 'done', reason: 'end_turn' },
  ]);

  /** Play a scripted answer: streamed text, two tool calls, a streamed summary. */
  const answer = (s: AiSession, prompt: string) => {
    const timers: ReturnType<typeof setTimeout>[] = [];
    runs.set(s.id, timers);
    const log = transcripts.get(s.id)!;
    let t = 250;
    const at = (ms: number, fn: () => void) => timers.push(setTimeout(fn, (t += ms)));
    const stream = (text: string) => {
      for (const word of text.split(/(?<= )/)) at(45, () => emit('ai.message', { session: s.id, role: 'assistant', text_delta: word }));
      at(30, () => {
        log.push({ type: 'assistant', text });
        emit('ai.message', { session: s.id, role: 'assistant', text });
      });
    };
    const tool = (name: string, args: unknown, ok: boolean, summary: string, ms: number) => {
      at(120, () => {
        log.push({ type: 'tool_call', name, args });
        emit('ai.tool_call', { session: s.id, name, args });
      });
      at(ms, () => {
        log.push({ type: 'tool_result', name, ok, summary });
        emit('ai.tool_result', { session: s.id, name, ok, summary });
      });
    };
    const short = prompt.length > 60 ? `${prompt.slice(0, 57)}…` : prompt;
    stream(`Sure — I'll work on "${short}". First I'll open a tab in my own group and read the page.`);
    const tab = 100 + sessionCount;
    tool('tabs_open', { url: 'https://shop.example.com/laptops', background: true }, true, `{"tab":${tab},"url":"https://shop.example.com/laptops","title":"Laptops · Example Shop"}`, 500);
    tool('page_read', { tab, max_tokens: 2000 }, true, 'Laptops · Example Shop — 24 products, filters: brand, price, RAM; table with 12 rows', 700);
    tool('page_find', { tab, query: 'pricing table' }, false, 'NOT_FOUND: no element matches "pricing table" (try page_snapshot)', 400);
    tool('page_snapshot', { tab, max_nodes: 400 }, true, 'table "Laptops" (12 rows): X1 Carbon $1,499 · T14s $1,129 · ZBook $1,890 …', 500);
    stream(
      'Here is what I found:\n\n| Laptop | Price |\n|---|--:|\n| ThinkPad X1 Carbon | $1,499 |\n| ThinkPad T14s | $1,129 |\n| HP ZBook | $1,890 |\n\nThe cheapest option with 32 GB RAM is the **T14s** ([details](https://shop.example.com/laptops/t14s)). Want me to compare shipping times too?',
    );
    at(60, () => {
      if (!s.tabs.includes(tab)) s.tabs.push(tab);
      log.push({ type: 'done', reason: 'end_turn' });
      setStatus(s, 'idle'); // like the browser: status first, then ai.done
      emit('ai.done', { session: s.id, reason: 'end_turn' });
      runs.delete(s.id);
      turnEnded(s);
    });
  };

  /** A turn is over: the agent takes the oldest queued message, if any, as its next turn. */
  const turnEnded = (s: AiSession) => {
    const queue = queues.get(s.id);
    const text = queue?.shift();
    if (text === undefined) return;
    s.queued = queue!.length;
    transcripts.get(s.id)!.push({ type: 'user', text, ts_ms: Date.now() });
    emit('ai.message', { session: s.id, role: 'user', text, taken: true });
    setStatus(s, 'running');
    publish(s);
    runs.set(s.id, [setTimeout(() => startTurn(s, text), 150)]);
  };

  /** Run the agent on `text` (already in the transcript). */
  const startTurn = (s: AiSession, text: string) => {
    setStatus(s, 'running');
    const asks = s.access && (s.provider === 'codex' || s.access.permission_mode === 'ask');
    if (asks && /\b(run|bash|command|files?)\b/i.test(text)) answerWithApproval(s, text);
    else answer(s, text);
  };

  /** Stop what runs now (timers, approval cards). */
  const halt = (s: AiSession) => {
    runs.get(s.id)?.forEach(clearTimeout);
    runs.delete(s.id);
    cancelApprovals(s.id);
  };

  // Approval requests (like crates/ai/src/cli/permission.rs): pending ones, and the tools a
  // session always allows.
  const approvals = new Map<string, { request: AiPermission; decide: (decision: AiPermissionDecision, reason?: string) => void }>();
  const alwaysAllowed = new Map<string, Set<string>>();
  let approvalCount = 0;
  const access = new Map<string, AiAgentAccess>();
  const accessOf = (id: string): AiAgentAccess =>
    access.get(id) ?? {
      provider: id,
      permission_mode: 'ask',
      cwd: null,
      default_cwd: 'C:\\BrowserMCP\\data\\ai\\workspace',
      add_dirs: [],
      sandbox: 'workspace_write',
      approval_policy: 'on_request',
    };

  /**
   * A CLI chat in "ask" mode asked to run something: a Bash call (Claude Code) or a shell
   * command (Codex) waits for the approval card, then runs or reports the refusal.
   */
  const answerWithApproval = (s: AiSession, prompt: string) => {
    const log = transcripts.get(s.id)!;
    const codex = s.provider === 'codex';
    const tool = codex ? 'shell' : 'Bash';
    const command = codex ? 'Remove-Item -Recurse build' : 'dir /s *.csv';
    const finish = (text: string) => {
      log.push({ type: 'assistant', text });
      emit('ai.message', { session: s.id, role: 'assistant', text });
      log.push({ type: 'done', reason: 'end_turn' });
      setStatus(s, 'idle');
      emit('ai.done', { session: s.id, reason: 'end_turn' });
      runs.delete(s.id);
      turnEnded(s);
    };
    const run = (ok: boolean, summary: string) => {
      log.push({ type: 'tool_result', name: tool, ok, summary });
      emit('ai.tool_result', { session: s.id, name: tool, ok, summary });
      setTimeout(() => finish(ok ? `Done: "${prompt}".` : 'Understood, I will not run it. Anything else?'), 400);
    };
    const timer = setTimeout(() => {
      const args = { command };
      log.push({ type: 'tool_call', name: tool, args });
      emit('ai.tool_call', { session: s.id, name: tool, args });
      if (alwaysAllowed.get(s.id)?.has(tool)) return void setTimeout(() => run(true, 'ok'), 500);
      const request: AiPermission = {
        id: `${s.id}.p${++approvalCount}`,
        session: s.id,
        tool,
        kind: codex ? 'command' : 'tool',
        title: codex ? null : `Claude wants to run ${command}`,
        reason: codex ? 'Needs to clean the build folder' : null,
        input: codex ? { command, cwd: accessOf(s.provider).default_cwd } : { command },
        status: 'pending',
        always: false,
        answer: null,
        created_ms: Date.now(),
      };
      s.pending_permissions = (s.pending_permissions ?? 0) + 1;
      emit('ai.permission', request);
      approvals.set(request.id, {
        request,
        decide: (decision, reason) => {
          approvals.delete(request.id);
          s.pending_permissions = Math.max(0, (s.pending_permissions ?? 1) - 1);
          if (decision === 'allow_always') {
            if (!alwaysAllowed.has(s.id)) alwaysAllowed.set(s.id, new Set());
            alwaysAllowed.get(s.id)!.add(tool);
          }
          const resolved: AiPermission = {
            ...request,
            status: decision === 'deny' ? 'denied' : 'allowed',
            always: decision === 'allow_always',
            answer: decision === 'deny' ? (reason ?? null) : null,
          };
          log.push({ type: 'permission', ...resolved });
          emit('ai.permission', resolved);
          if (decision === 'deny') run(false, reason ? `The user denied this action: ${reason}` : 'The user denied this action.');
          else setTimeout(() => run(true, codex ? 'removed 14 files' : '3 files found'), 500);
        },
      });
    }, 500);
    runs.set(s.id, [timer]);
  };

  /** Cancel the approval requests of a session that stops or closes. */
  const cancelApprovals = (id: string) => {
    for (const [key, { request }] of approvals) {
      if (request.session !== id) continue;
      approvals.delete(key);
      const cancelled: AiPermission = { ...request, status: 'cancelled' };
      transcripts.get(id)?.push({ type: 'permission', ...cancelled });
      emit('ai.permission', cancelled);
    }
    const s = sessions.get(id);
    if (s) s.pending_permissions = 0;
  };

  /** A user message: a new turn, or queued while the agent works (never refused). */
  const sendUser = (s: AiSession, text: string) => {
    if (s.status === 'running') {
      const queue = queues.get(s.id) ?? [];
      queues.set(s.id, queue);
      queue.push(text);
      s.queued = queue.length;
      emit('ai.message', { session: s.id, role: 'user', text, queued: true });
      publish(s);
      return;
    }
    transcripts.get(s.id)!.push({ type: 'user', text, ts_ms: Date.now() });
    if (userCount(s.id) === 1) retitle(s);
    emit('ai.message', { session: s.id, role: 'user', text });
    startTurn(s, text);
    publish(s);
  };

  // ---- terminals ---------------------------------------------------------------------------
  const terms = new Map<string, MockTerm>();
  let termCount = 0;
  const out = (id: string, data: string) => emit('term.output', { id, data });

  const runCommand = (t: MockTerm, cmd: string) => {
    const id = t.info.id;
    const [name, ...rest] = cmd.trim().split(/\s+/);
    switch ((name ?? '').toLowerCase()) {
      case '':
        break;
      case 'help':
        out(id, 'Mock shell (no backend). Try: dir, echo <text>, date, colors, clear, exit\r\n');
        break;
      case 'dir':
      case 'ls':
        out(id, '\r\n    Directory: C:\\Users\\you\r\n\r\n\x1b[1mMode   LastWriteTime      Length Name\x1b[0m\r\n');
        for (const [mode, name2, len] of [
          ['d----', 'Desktop', ''],
          ['d----', 'Documents', ''],
          ['d----', 'Downloads', ''],
          ['-a---', 'notes.txt', '1204'],
          ['-a---', 'prices.csv', '48211'],
        ])
          out(id, `${mode}  11/6/2026  9:41 AM ${len.padStart(8)} ${mode.startsWith('d') ? `\x1b[34;1m${name2}\x1b[0m` : name2}\r\n`);
        out(id, '\r\n');
        break;
      case 'echo':
        out(id, `${rest.join(' ')}\r\n`);
        break;
      case 'date':
        out(id, `${new Date().toString()}\r\n`);
        break;
      case 'colors':
        out(id, [31, 32, 33, 34, 35, 36].map((c) => `\x1b[${c}m██ ${c}\x1b[0m`).join(' ') + '\r\n');
        break;
      case 'clear':
      case 'cls':
        out(id, '\x1b[2J\x1b[H');
        break;
      case 'exit':
        t.info.exit_code = 0;
        emit('term.exit', { id, code: 0 });
        return;
      default:
        out(id, `\x1b[31m${name}: The term '${name}' is not recognized as a command in the mock shell.\x1b[0m\r\n`);
    }
    out(id, shellPrompt(t.info.shell));
  };

  const requireTerm = (id: string) => {
    const t = terms.get(id);
    if (!t) throw new RpcError('NOT_FOUND', `no terminal ${id}`);
    return t;
  };

  // ---- traffic -------------------------------------------------------------------------------
  const traffic = new Map<TabId, TabTraffic>();
  const rules: MockRule[] = [];
  const pauses = new Map<string, MockPause>();
  let nextEntry = 1;
  let nextRule = 1;
  let nextPause = 1;

  const tabOf = (tab: TabId | undefined): TabId => {
    const t = tab ?? activeTab();
    if (t === null) throw new RpcError('NO_TABS', 'no tab is open');
    return t;
  };
  const trafficOf = (tab: TabId): TabTraffic => {
    let t = traffic.get(tab);
    if (!t) traffic.set(tab, (t = { entries: [], capture: false, timer: null }));
    return t;
  };
  const requireCaptured = (tab: TabId): TabTraffic => {
    const t = trafficOf(tab);
    if (!t.capture && t.entries.length === 0) {
      throw new RpcError('NOT_FOUND', `nothing captured on tab ${tab}`);
    }
    return t;
  };
  const entryOf = (tab: TabId, id: number): Entry => {
    const e = trafficOf(tab).entries.find((x) => x.id === id);
    if (!e) throw new RpcError('NOT_FOUND', `no request #${id} on tab ${tab}`);
    return e;
  };

  const finish = (e: Entry, status: number, size: number, body: string | null, mime: string) => {
    e.status = status;
    e.statusText = status === 200 ? 'OK' : status === 201 ? 'Created' : status === 204 ? 'No Content' : status === 304 ? 'Not Modified' : status === 404 ? 'Not Found' : status === 500 ? 'Internal Server Error' : '';
    e.size = size;
    e.end = Date.now();
    e.resBody = body;
    e.mime = mime;
    e.resHeaders = [
      ['content-type', mime],
      ['content-length', String(size)],
      ['cache-control', 'private, max-age=0'],
      ['date', new Date().toUTCString()],
      ['server', 'mock'],
    ];
  };

  const ruleFor = (tab: TabId, e: Entry, stage: NetStage) =>
    rules.find((r) => r.tab === tab && r.stage === stage && (!r.method || r.method === e.method) && r.regex.test(e.url));

  const pause = (tab: TabId, rule: MockRule, e: Entry, stage: NetStage, done: (edits: NetEdits | null) => void) => {
    const pauseId = `p${nextPause++}`;
    const info: NetPaused = {
      pause_id: pauseId,
      tab,
      rule: rule.id,
      stage,
      method: e.method,
      url: e.url,
      resource_type: e.type,
      headers: Object.fromEntries(e.reqHeaders),
      ...(e.reqBody ? { post_data: e.reqBody } : {}),
      ...(stage === 'response'
        ? { status: e.status ?? 200, response_headers: e.resHeaders.map(([name, value]) => ({ name, value })) }
        : {}),
    };
    const timer = setTimeout(() => {
      pauses.delete(pauseId);
      emit('net.pause_timeout', { pause_id: pauseId, tab, url: e.url, warning: `${pauseId} was not resumed within 60 s and continued unchanged` });
      done(null);
    }, 60_000);
    pauses.set(pauseId, { info, entry: e, deadline: Date.now() + 60_000, timer, done });
    emit('net.paused', info);
  };

  const request = (tab: TabId, spec: (typeof PAGE_LOAD)[number]) => {
    const t = trafficOf(tab);
    if (!t.capture) return;
    const [method, url, type, mime, status, size, ms, body] = spec;
    const e: Entry = {
      id: nextEntry++,
      method,
      url,
      type,
      mime: '',
      status: null,
      statusText: '',
      error: null,
      size: 0,
      start: Date.now(),
      end: null,
      fromCache: status === 304,
      reqHeaders: [
        [':authority', new URL(url).host],
        ['accept', type === 'document' ? 'text/html,application/xhtml+xml' : '*/*'],
        ['user-agent', 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) BrowserMCP/0.1'],
        ['cookie', 'sid=9c1f0e; theme=dark'],
        ...(method === 'POST' ? ([['content-type', 'application/json']] as [string, string][]) : []),
      ],
      resHeaders: [],
      reqBody: method === 'POST' ? JSON.stringify({ event: 'view', product: 'x1-carbon', ts: Date.now() }) : null,
      resBody: null,
      frames: [],
      notes: [],
    };
    t.entries.push(e);
    if (t.entries.length > 5000) t.entries.shift();

    const respond = () => {
      setTimeout(() => {
        if (e.error) return;
        const st = status;
        let b = body ?? null;
        const resRule = ruleFor(tab, e, 'response');
        finish(e, st, size, b, mime);
        if (resRule) {
          resRule.hits++;
          if (resRule.action === 'pause') {
            // Like CEF: a server response keeps its status and headers (net.resume refuses
            // changing them), only the body can be replaced.
            pause(tab, resRule, e, 'response', (edits) => {
              if (edits?.response_body !== undefined) b = edits.response_body;
              finish(e, st, size, b, mime);
              if (edits) e.notes.push(`${resRule.id} edited`);
            });
            // Held: shows as pending until resumed.
            e.end = null;
            e.status = null;
          } else if (typeof resRule.action === 'object' && 'rewrite_body' in resRule.action && b) {
            e.resBody = b.split(resRule.action.rewrite_body.find).join(resRule.action.rewrite_body.replace);
            e.notes.push(`${resRule.id} rewrite_body`);
          }
        }
      }, ms);
    };

    const reqRule = ruleFor(tab, e, 'request');
    if (!reqRule) return respond();
    reqRule.hits++;
    const a = reqRule.action;
    if (a === 'block') {
      e.error = 'net::ERR_BLOCKED_BY_CLIENT';
      e.end = Date.now();
      e.notes.push(`${reqRule.id} block`);
    } else if (a === 'pause') {
      pause(tab, reqRule, e, 'request', (edits) => {
        if (edits?.url) e.url = edits.url;
        if (edits?.method) e.method = edits.method;
        if (edits) e.notes.push(`${reqRule.id} edited`);
        respond();
      });
    } else if ('mock' in a) {
      setTimeout(() => {
        const mockBody = a.mock.body ?? '';
        finish(e, a.mock.status ?? 200, mockBody.length, mockBody, a.mock.headers?.['content-type'] ?? 'text/plain');
        e.notes.push(`${reqRule.id} mock ${a.mock.status ?? 200}`);
      }, 5);
    } else if ('redirect' in a) {
      e.notes.push(`${reqRule.id} redirect → ${a.redirect.url}`);
      respond();
    } else {
      e.notes.push(`${reqRule.id} ${actionShort(a).split(' ')[0]}`);
      respond();
    }
  };

  const openSocket = (tab: TabId) => {
    const t = trafficOf(tab);
    const e: Entry = {
      id: nextEntry++,
      method: 'GET',
      url: 'wss://live.shop.example.com/socket?v=2',
      type: 'websocket',
      mime: '',
      status: 101,
      statusText: 'Switching Protocols',
      error: null,
      size: 0,
      start: Date.now(),
      end: null,
      fromCache: false,
      reqHeaders: [['upgrade', 'websocket'], ['sec-websocket-version', '13']],
      resHeaders: [['upgrade', 'websocket'], ['connection', 'Upgrade']],
      reqBody: null,
      resBody: null,
      frames: [],
      notes: [],
    };
    t.entries.push(e);
    e.frames.push({ sent: true, opcode: 1, data: '{"op":"subscribe","topics":["stock","cart"]}', ts: Date.now() });
    return e;
  };

  const startTraffic = (tab: TabId) => {
    const t = trafficOf(tab);
    PAGE_LOAD.forEach((spec, i) => setTimeout(() => request(tab, spec), i * 60));
    let socket: Entry | null = null;
    setTimeout(() => (socket = t.capture ? openSocket(tab) : null), 700);
    let n = 0;
    t.timer = setInterval(() => {
      if (!t.capture) return;
      request(tab, LIVE[n++ % LIVE.length]);
      if (socket) {
        socket.frames.push({
          sent: false,
          opcode: 1,
          data: JSON.stringify({ op: 'stock', id: 'x1-carbon', qty: Math.floor(Math.random() * 20) }),
          ts: Date.now(),
        });
        if (n % 3 === 0) socket.frames.push({ sent: true, opcode: 9, data: '', ts: Date.now() });
      }
    }, 1800);
  };

  return {
    // ---- ai ----
    ...providerMocks.handlers,
    'ai.session_start': ({ provider: pid, prompt, model, permission_mode, cwd, sandbox, approval_policy, add_dirs }) => {
      const p = getProvider(pid);
      if (!prompt.trim()) throw new RpcError('INVALID_PARAMS', 'prompt is empty');
      if (!p.configured) throw new RpcError('INVALID_PARAMS', `AI provider '${pid}' is not configured`);
      if (cwd && /missing|nope/i.test(cwd)) throw new RpcError('INVALID_PARAMS', `folder not found: ${cwd}`);
      sessionCount++;
      const defaults = accessOf(pid);
      const s: AiSession = {
        id: `${pid}-${sessionCount}`,
        provider: pid,
        model: model ?? p.model,
        color: AGENT_COLORS[(sessionCount - 1) % AGENT_COLORS.length],
        status: 'idle',
        tabs: [],
        created_ms: Date.now(),
        access:
          p.kind === 'cli'
            ? {
                permission_mode: permission_mode ?? defaults.permission_mode,
                cwd: cwd || defaults.cwd || defaults.default_cwd,
                // Unset = the provider's; [] = none (as the core does).
                add_dirs: add_dirs ?? defaults.add_dirs,
                sandbox: sandbox ?? defaults.sandbox,
                approval_policy: approval_policy ?? defaults.approval_policy,
              }
            : null,
        pending_permissions: 0,
        title: p.name,
        title_set: false,
        archived: false,
        updated_ms: Date.now(),
        queued: 0,
      };
      sessions.set(s.id, s);
      transcripts.set(s.id, []);
      sendUser(s, prompt);
      return { ...s };
    },
    // Like the core: a stopped or failed chat is continued, never replaced; while it works the
    // message is queued (never refused).
    'ai.session_send': ({ id, text }) => {
      const s = session(id);
      if (s.status === 'stopped' || s.status === 'failed') setStatus(s, 'idle');
      sendUser(s, text);
      return { ...s };
    },
    'ai.session_stop': ({ id }) => {
      const s = session(id);
      halt(s);
      queues.delete(id);
      s.queued = 0;
      if (s.status === 'running') {
        transcripts.get(id)!.push({ type: 'done', reason: 'stopped' });
        setStatus(s, 'stopped');
        emit('ai.done', { session: id, reason: 'stopped' });
      } else setStatus(s, 'stopped');
      return { ...s };
    },
    'ai.session_interrupt': ({ id }) => {
      const s = session(id);
      if (s.status !== 'running') return { ...s };
      halt(s);
      transcripts.get(id)!.push({ type: 'done', reason: 'interrupted' });
      setStatus(s, 'idle');
      emit('ai.done', { session: id, reason: 'interrupted' });
      turnEnded(s);
      return { ...s };
    },
    // Close = archive: every file stays; History lists it.
    'ai.session_close': ({ id }) => {
      const s = session(id);
      halt(s);
      queues.delete(id);
      s.queued = 0;
      s.archived = true;
      if (s.status === 'running') {
        transcripts.get(id)!.push({ type: 'done', reason: 'stopped' });
        s.status = 'stopped';
      }
      s.updated_ms = Date.now();
      emit('ai.session_closed', { session: id });
      return { closed: id };
    },
    'ai.session_reopen': ({ id }) => {
      const s = session(id);
      s.archived = false;
      s.updated_ms = Date.now();
      publish(s);
      return { ...s };
    },
    'ai.session_delete': ({ id }) => {
      const s = session(id);
      halt(s);
      queues.delete(id);
      sessions.delete(id);
      transcripts.delete(id);
      emit('ai.session_closed', { session: id });
      return { deleted: true };
    },
    'ai.session_rename': ({ id, title }) => {
      const s = session(id);
      const name = title.replace(/\s+/g, ' ').trim();
      s.title_set = name !== '';
      if (name) s.title = name;
      else retitle(s);
      publish(s);
      return { ...s };
    },
    'ai.models': ({ provider }) => {
      const p = getProvider(provider);
      if (p.protocol === 'claude-code') {
        return {
          default: 'default',
          models: [
            { id: 'default', name: 'Default', hint: 'Your account’s default model' },
            { id: 'fable', name: 'Fable', hint: 'Latest Fable · claude-fable-5-1' },
            { id: 'opus', name: 'Opus', hint: 'Latest Opus · claude-opus-5-5' },
            { id: 'sonnet', name: 'Sonnet', hint: 'Latest Sonnet · claude-sonnet-5-5' },
            { id: 'haiku', name: 'Haiku', hint: 'Latest Haiku · claude-haiku-5-5' },
            { id: 'claude-fable-5-1', name: 'claude-fable-5-1' },
            { id: 'claude-opus-5-5', name: 'claude-opus-5-5' },
            { id: 'claude-sonnet-5-5', name: 'claude-sonnet-5-5' },
            { id: 'claude-haiku-5-5', name: 'claude-haiku-5-5' },
          ],
        };
      }
      if (p.protocol === 'codex') {
        return {
          default: 'gpt-5-codex',
          models: [
            { id: 'gpt-5-codex', name: 'gpt-5-codex', hint: 'Tuned for coding' },
            { id: 'gpt-5', name: 'gpt-5' },
            { id: 'gpt-5-mini', name: 'gpt-5-mini', hint: 'Faster, cheaper' },
          ],
        };
      }
      const ids = p.models.length ? p.models : p.model ? [p.model] : [];
      return { default: p.model ?? undefined, models: ids.map((m) => ({ id: m, name: m })) };
    },
    'ai.session_set_model': ({ id, model }) => {
      const s = session(id);
      if (!model.trim()) throw new RpcError('INVALID_PARAMS', 'model is empty');
      s.model = model.trim();
      publish(s);
      return { ...s };
    },
    // Rewind to just before user message `index` and send `text` instead.
    'ai.session_edit': ({ id, index, text }) => {
      const s = session(id);
      if (!text.trim()) throw new RpcError('INVALID_PARAMS', 'text is empty');
      const log = transcripts.get(id)!;
      let n = -1;
      const at = log.findIndex((e) => e.type === 'user' && ++n === index);
      if (at < 0) throw new RpcError('INVALID_PARAMS', `chat '${id}' has no user message #${index}`);
      halt(s);
      queues.delete(id);
      s.queued = 0;
      log.splice(at);
      if (s.status !== 'idle') setStatus(s, 'idle');
      sendUser(s, text.trim()); // a first message renames an untouched chat
      return { ...s };
    },
    'ai.permissions_list': ({ session } = {}) => ({
      permissions: [...approvals.values()].map((a) => a.request).filter((r) => !session || r.session === session),
    }),
    'ai.permission_answer': ({ id, decision, reason }) => {
      const pending = approvals.get(id);
      if (!pending) throw new RpcError('NOT_FOUND', `no pending approval request '${id}'`);
      pending.decide(decision, reason ?? undefined);
      const resolved: AiPermission = {
        ...pending.request,
        status: decision === 'deny' ? 'denied' : 'allowed',
        always: decision === 'allow_always',
        answer: decision === 'deny' ? (reason ?? null) : null,
      };
      return { resolved: [resolved] };
    },
    'ai.agent_access': ({ id }) => ({ ...accessOf(getProvider(id).id) }),
    'ai.agent_access_set': ({ id, permission_mode, cwd, add_dirs, sandbox, approval_policy }) => {
      const next = { ...accessOf(getProvider(id).id) };
      if (cwd !== undefined && cwd !== null) {
        if (/missing|nope/i.test(cwd)) throw new RpcError('INVALID_PARAMS', `folder not found: ${cwd}`);
        next.cwd = cwd.trim() || null;
      }
      if (permission_mode) next.permission_mode = permission_mode;
      if (add_dirs) next.add_dirs = add_dirs.filter((d) => d.trim());
      if (sandbox) next.sandbox = sandbox;
      if (approval_policy) next.approval_policy = approval_policy;
      access.set(id, next);
      return { ...next };
    },
    'ai.sessions_list': ({ archived } = {}) => {
      const list = [...sessions.values()].filter((s) => !!s.archived === !!archived).map((s) => ({ ...s }));
      if (archived) list.sort((a, b) => (b.updated_ms ?? b.created_ms) - (a.updated_ms ?? a.created_ms));
      return { sessions: list };
    },
    'ai.session_transcript': ({ id }) => {
      const entries = transcripts.get(id);
      if (!entries) throw new RpcError('NOT_FOUND', `no AI session '${id}'`);
      return { id, entries };
    },

    // ---- term ----
    'term.open': ({ shell, cols, rows }) => {
      const name = shell ?? 'pwsh';
      if (name === 'bash' && termCount % 2 === 1) throw new RpcError('INTERNAL', 'cannot start bash: program not found');
      const program = name === 'pwsh' ? 'pwsh.exe' : name === 'cmd' ? 'cmd.exe' : name;
      const info: TermInfo = { id: `t${++termCount}`, shell: program, cwd: 'C:\\Users\\you', cols: cols ?? 80, rows: rows ?? 24, pid: 4000 + termCount, exit_code: null };
      const t: MockTerm = { info, line: '' };
      terms.set(info.id, t);
      setTimeout(() => {
        const banner = program.startsWith('pwsh')
          ? 'PowerShell 7.5.2 (mock)\r\n'
          : program.startsWith('cmd')
            ? 'Microsoft Windows [Version 10.0.26100] (mock)\r\n\r\n'
            : '';
        out(info.id, `${banner}Type \x1b[1mhelp\x1b[0m for the mock commands.\r\n${shellPrompt(program)}`);
      }, 80);
      return { ...info };
    },
    'term.write': ({ id, data }) => {
      const t = requireTerm(id);
      if (t.info.exit_code !== null) throw new RpcError('NOT_FOUND', `terminal ${id} has exited`);
      for (const ch of data) {
        if (ch === '\r') {
          out(id, '\r\n');
          const cmd = t.line;
          t.line = '';
          runCommand(t, cmd);
        } else if (ch === '\x7f' || ch === '\b') {
          if (t.line) {
            t.line = t.line.slice(0, -1);
            out(id, '\b \b');
          }
        } else if (ch === '\x03') {
          t.line = '';
          out(id, `^C\r\n${shellPrompt(t.info.shell)}`);
        } else if (ch >= ' ' && ch !== '\x1b') {
          t.line += ch;
          out(id, ch);
        }
      }
      return {};
    },
    'term.resize': ({ id, cols, rows }) => {
      const t = requireTerm(id);
      t.info.cols = cols;
      t.info.rows = rows;
      return {};
    },
    'term.close': ({ id }) => {
      const t = requireTerm(id);
      terms.delete(id);
      if (t.info.exit_code === null) emit('term.exit', { id, code: 1 });
      return {};
    },
    'term.list': () => ({ terminals: [...terms.values()].map((t) => ({ ...t.info })) }),

    // ---- net ----
    'net.capture': ({ tab, on }) => {
      const id = tabOf(tab);
      const t = trafficOf(id);
      if (on && !t.capture) {
        t.capture = true;
        startTraffic(id);
      } else if (!on) {
        t.capture = false;
        if (t.timer) clearInterval(t.timer);
        t.timer = null;
      }
      return { tab: id, capturing: t.capture, bodies: false, entries: t.entries.length };
    },
    'net.list': ({ tab, filter, cursor, limit }) => {
      const id = tabOf(tab);
      const t = requireCaptured(id);
      const max = Math.min(Math.max(limit ?? 50, 1), 500);
      const from = cursor ?? 0;
      const matching = t.entries.filter((e) => e.id >= from && filterMatches(filter, e));
      const shown = matching.slice(0, max);
      const next_cursor = shown.length ? shown[shown.length - 1].id + 1 : Math.max(from, nextEntry);
      const more = matching.length > shown.length;
      let text = `tab ${id}: ${t.entries.length} captured${t.capture ? '' : ' (capture off)'}, ${matching.length} match`;
      for (const e of shown) text += `\n${line(e)}`;
      if (more) text += `\n… ${matching.length - shown.length} more: net_list cursor=${next_cursor}`;
      return { tab: id, total: matching.length, text, next_cursor, more };
    },
    'net.get': ({ tab, id, parts, max_bytes }) => {
      const tid = tabOf(tab);
      const e = entryOf(tid, id);
      const want = parts ?? ['headers'];
      const max = Math.max(max_bytes ?? 4096, 16);
      const facts = [
        e.error ? `failed ${e.error}` : e.status !== null ? `status ${e.status} ${e.statusText}` : 'pending',
        e.type,
        ...(e.mime ? [e.mime] : []),
        ...(e.size ? [fmtSize(e.size)] : []),
        ...(e.end !== null ? [fmtDuration(e.end - e.start)] : []),
        'h2',
        '203.0.113.7',
        ...(e.fromCache ? ['from cache'] : []),
      ];
      let text = `#${e.id} ${e.method} ${e.url}\n${facts.join(' · ')}\ninitiator: ${e.type === 'document' ? 'other' : 'script https://shop.example.com/assets/app-4f2a91.js:1'}`;
      if (e.notes.length) text += `\nnotes: ${e.notes.join('; ')}`;
      if (e.type === 'websocket') text += `\nframes: ${e.frames.length} (net_ws_frames id=${e.id})`;
      let truncated = false;
      const body = (label: string, b: string | null, missing: string) => {
        if (b === null) return `\n${label}: ${missing}`;
        const cut = b.length > max;
        truncated ||= cut;
        return `\n${label} (${fmtSize(b.length)}${cut ? `, cut to ${max}B` : ''}):\n${cut ? b.slice(0, max) : b}`;
      };
      if (want.includes('headers')) {
        for (const [title, hs] of [
          ['request headers', e.reqHeaders],
          ['response headers', e.resHeaders],
        ] as const) {
          text += `\n${title} (${hs.length}):`;
          for (const [n, v] of hs) text += `\n  ${n}: ${v}`;
        }
      }
      if (want.includes('req_body')) text += body('request body', e.reqBody, 'none');
      if (want.includes('res_body')) {
        const why = e.type === 'websocket' ? 'websockets have frames, not a body (use net_ws_frames)' : e.end === null ? 'still loading' : e.type === 'image' || e.type === 'font' ? `[binary ${fmtSize(e.size)} ${e.mime}, not shown]` : 'not available (No resource with given identifier found)';
        text += body('response body', e.resBody, why);
      }
      return { tab: tid, id, text, truncated };
    },
    'net.search': ({ tab, query }) => {
      const id = tabOf(tab);
      const t = requireCaptured(id);
      const q = query.toLowerCase();
      const hits = t.entries.filter((e) => e.url.toLowerCase().includes(q) || (e.resBody ?? '').toLowerCase().includes(q));
      return { tab: id, total: hits.length, text: hits.length ? hits.map(line).join('\n') : `no request contains ${JSON.stringify(query)}` };
    },
    'net.ws_frames': ({ tab, id, cursor, limit, max_bytes }) => {
      const tid = tabOf(tab);
      const e = entryOf(tid, id);
      if (e.type !== 'websocket') throw new RpcError('INVALID_PARAMS', `#${e.id} is a ${e.type} request, not a websocket`);
      const first = cursor ?? 0;
      const max = Math.max(max_bytes ?? 300, 16);
      const lim = Math.min(Math.max(limit ?? 50, 1), 500);
      const kinds: Record<number, string> = { 1: 'text', 2: 'binary', 8: 'close', 9: 'ping', 10: 'pong' };
      let text = `#${e.id} websocket ${e.url} — ${e.frames.length} frames`;
      let next = first;
      e.frames.slice(first, first + lim).forEach((f, i) => {
        const n = first + i;
        const payload = f.data.length > max ? `${f.data.slice(0, max)}…` : f.data;
        text += `\n${n} ${f.sent ? '→' : '←'} ${kinds[f.opcode] ?? 'frame'} ${fmtSize(f.data.length)} +${((f.ts - e.start) / 1000).toFixed(3)}s ${payload}`;
        next = n + 1;
      });
      return { tab: tid, id, total: e.frames.length, text, next_cursor: next, more: next < e.frames.length };
    },
    'net.har_export': ({ tab }) => {
      const id = tabOf(tab);
      const t = requireCaptured(id);
      const entries = t.entries.filter((e) => e.type !== 'websocket');
      const har = {
        log: {
          version: '1.2',
          creator: { name: 'BrowserMCP', version: '0.1.0' },
          entries: entries.map((e) => ({
            startedDateTime: new Date(e.start).toISOString(),
            time: e.end !== null ? e.end - e.start : -1,
            request: { method: e.method, url: e.url, headers: e.reqHeaders.map(([name, value]) => ({ name, value })) },
            response: { status: e.status ?? 0, headers: e.resHeaders.map(([name, value]) => ({ name, value })), content: { size: e.size, mimeType: e.mime, text: e.resBody ?? undefined } },
          })),
        },
      };
      const bytes = JSON.stringify(har, null, 2).length;
      if (bytes > 16 * 1024) return { tab: id, entries: entries.length, bytes, path: `C:\\Users\\you\\BrowserMCP\\data\\har\\tab${id}-${Date.now()}.har` };
      return { tab: id, entries: entries.length, bytes, har };
    },
    'net.rule_add': ({ tab, match, action }) => {
      const id = tabOf(tab);
      const r = makeRule(id, match, action, undefined, `r${nextRule++}`);
      rules.push(r);
      return { id: r.id, text: ruleLine(r) };
    },
    'net.breakpoint': ({ tab, match, stage }) => {
      const id = tabOf(tab);
      const r = makeRule(id, match ?? {}, 'pause', stage, `b${nextRule++}`);
      rules.push(r);
      return { id: r.id, text: `${ruleLine(r)}\npaused requests: net.paused events and net_paused_list; answer with net_resume` };
    },
    'net.rule_list': ({ tab }) => {
      const lines = rules.filter((r) => tab === undefined || r.tab === tab).map(ruleLine);
      return { count: lines.length, text: lines.length ? lines.join('\n') : 'no rules or breakpoints' };
    },
    'net.rule_remove': ({ id }) => {
      const i = rules.findIndex((r) => r.id === id);
      if (i < 0) throw new RpcError('NOT_FOUND', `no rule ${id}`);
      const [r] = rules.splice(i, 1);
      let released = 0;
      for (const [pid, p] of pauses) {
        if (p.info.rule !== id) continue;
        clearTimeout(p.timer);
        pauses.delete(pid);
        p.done(null);
        released++;
      }
      return { id, text: `removed ${ruleLine(r)}${released ? `; continued ${released} paused requests` : ''}` };
    },
    'net.paused_list': ({ tab }) => {
      const lines = [...pauses.values()]
        .filter((p) => tab === undefined || p.info.tab === tab)
        .map((p) => {
          const left = Math.max(0, Math.floor((p.deadline - Date.now()) / 1000));
          const status = p.info.stage === 'response' && p.info.status ? ` ${p.info.status}` : '';
          return `${p.info.pause_id} ${p.info.rule} tab ${p.info.tab} ${p.info.stage} ${p.info.method}${status} ${shortUrl(p.info.url)} (${left}s left)`;
        });
      return { count: lines.length, text: lines.length ? lines.join('\n') : 'nothing is paused' };
    },
    'net.resume': ({ pause_id, edits, action }) => {
      const p = pauses.get(pause_id);
      if (!p) throw new RpcError('NOT_FOUND', `no paused request ${pause_id}`);
      if (p.info.stage === 'response' && (edits?.url || edits?.method || edits?.body !== undefined)) {
        throw new RpcError('INVALID_PARAMS', 'this request is paused at the response stage: url, method and body were already sent; edit status, headers or response_body instead');
      }
      if (action !== 'drop' && p.info.stage === 'response') {
        const problem = responseProblem(p.info, edits ?? {});
        if (problem) throw new RpcError('INVALID_PARAMS', problem);
      }
      clearTimeout(p.timer);
      pauses.delete(pause_id);
      if (action === 'drop') {
        p.entry.error = 'net::ERR_ABORTED';
        p.entry.end = Date.now();
        p.entry.notes.push(`${p.info.rule} dropped`);
        return { pause_id, done: 'dropped', text: `dropped ${pause_id} ${p.info.method} ${p.info.url}` };
      }
      p.done(edits ?? {});
      const done = edits?.response_body !== undefined || (p.info.stage === 'request' && edits?.status) ? 'fulfilled' : 'continued';
      return { pause_id, done, text: `${done} ${pause_id} ${p.info.method} ${p.info.url}` };
    },
  };

  function makeRule(tab: TabId, match: NetMatch, action: NetAction | 'pause', stage: NetStage | undefined, id: string): MockRule {
    if (match.url_glob && match.url_regex) throw new RpcError('INVALID_PARAMS', 'give either match.url_glob or match.url_regex, not both');
    let regex: RegExp;
    let what: string;
    if (match.url_regex) {
      try {
        regex = new RegExp(match.url_regex);
      } catch (err) {
        throw new RpcError('INVALID_PARAMS', `match.url_regex is not a valid regex: ${(err as Error).message}`);
      }
      what = `regex ${JSON.stringify(match.url_regex)}`;
    } else if (match.url_glob) {
      regex = globRegex(match.url_glob);
      what = `glob ${JSON.stringify(match.url_glob)}`;
    } else {
      regex = /.*/;
      what = 'any url';
    }
    const method = match.method ? match.method.trim().toUpperCase() : null;
    const st = stage ?? match.stage ?? 'request';
    if (action !== 'pause' && typeof action === 'object' && 'redirect' in action && st === 'response') {
      throw new RpcError('INVALID_PARAMS', 'redirect only works at the request stage (the response already came from the original URL)');
    }
    // Like rules::validate: CEF gives the page the server's status and headers after a
    // response-stage answer, so these would silently do nothing.
    if (action !== 'pause' && typeof action === 'object' && st === 'response') {
      if ('set_headers' in action || 'remove_headers' in action) {
        throw new RpcError(
          'INVALID_PARAMS',
          "set_headers/remove_headers cannot change response headers: at the response stage the browser (CEF) gives the page the server's headers whatever the answer says",
        );
      }
      if ('mock' in action) {
        throw new RpcError(
          'INVALID_PARAMS',
          "mock only works at the request stage: at the response stage the browser (CEF) keeps the server's status and headers; a request-stage mock answers without sending the request",
        );
      }
    }
    return { id, tab, stage: st, regex, method, describe: method ? `${method} ${what}` : what, action, hits: 0 };
  }
}
