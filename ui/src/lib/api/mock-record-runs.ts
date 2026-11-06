// Fakes for the Recording and Runs panels (record.*, script.*, run.*, trigger.*), used by MockApi
// so `pnpm dev` demos a live recording, codegen, mass runs with progress, a heal request and
// cron triggers without a backend. Shapes follow the generated protocol types; behaviour is
// only plausible, not a spec (crates/record and crates/jobs are the truth).

import { RpcError } from './client';
import type { ApiEvents, EventName, Params, Result, TabId } from './types';
import type {
  CodegenInput,
  HealRequest,
  RecordRunsMethods,
  RecordStep,
  RecordingDetail,
  RowStatus,
  RunInfo,
  RunRow,
  TriggerInfo,
} from './types-record-runs';
import { nextFires, parseCron } from '../runs/cron';

type Emit = <E extends EventName>(event: E, payload: ApiEvents[E]) => void;
export type RecordRunsHandlers = {
  [M in keyof RecordRunsMethods]: (params: Params<M>) => Result<M> | Promise<Result<M>>;
};

const DATA = 'C:\\Users\\you\\AppData\\Roaming\\BrowserMCP\\data';
const MIN = 60_000;

interface Recording {
  meta: RecordingDetail['meta'];
  steps: RecordStep[];
}

interface MockRun {
  info: RunInfo;
  rows: RunRow[];
  /** Row index that breaks with a selector error (heal demo), until a patch is applied. */
  breakAt: number | null;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function stamp(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}`;
}

/** What a person does while the mock records: one step every ~1.4 s. */
const LIVE_SCRIPT: Array<Omit<RecordStep, 'seq' | 'ts_ms'>> = [
  { source: 'human', action: 'click', target: { role: 'link', name: 'Sign in', tag: 'a', from: 'dom' } },
  { source: 'human', action: 'navigated', url: 'https://shop.example.test/login' },
  { source: 'human', action: 'fill', target: { role: 'textbox', name: 'Email', tag: 'input', input_type: 'email' }, value: 'ann@example.test' },
  { source: 'human', action: 'fill', target: { role: 'textbox', name: 'Password', tag: 'input', input_type: 'password' }, value: '***', secret: true },
  { source: 'human', action: 'click', target: { role: 'button', name: 'Sign in', tag: 'button' } },
  { source: 'human', action: 'navigated', url: 'https://shop.example.test/account' },
  { source: 'api', action: 'command', command: { name: 'page.read', call_id: 41, caller: { kind: 'mcp', session: 's1', agent: 'claude' }, args: {}, ok: true, duration_ms: 84 } },
  { source: 'human', action: 'fill', target: { role: 'searchbox', name: 'Search products' }, value: 'thinkpad x1' },
  { source: 'human', action: 'press', target: { role: 'searchbox', name: 'Search products' }, value: 'Enter' },
  { source: 'human', action: 'select', target: { role: 'combobox', name: 'Sort by' }, value: 'Price: low to high' },
  { source: 'human', action: 'click', target: { role: 'link', name: 'ThinkPad X1 Carbon Gen 13', nth: 1 } },
];

function seedRecordings(now: number): Recording[] {
  const mk = (id: string, name: string, start: string, started: number, steps: Array<Omit<RecordStep, 'seq' | 'ts_ms'>>, video: boolean): Recording => ({
    meta: {
      id,
      name,
      tab: 1,
      started_ms: started,
      stopped_ms: started + steps.length * 2100,
      start_url: start,
      steps: steps.length + 1,
      human_capture: 'binding',
      video: video ? `${DATA}\\recordings\\${id}\\video.webm` : null,
      traffic: null,
    },
    steps: [
      { seq: 1, ts_ms: started, source: 'system', action: 'start', url: start },
      ...steps.map((s, i) => ({ ...s, seq: i + 2, ts_ms: started + (i + 1) * 2100 })),
    ],
  });
  const t1 = now - 26 * 60 * MIN;
  const t2 = now - 3 * 60 * MIN;
  return [
    mk(`search-${stamp(t2)}`, 'search', 'https://shop.example.test/', t2, LIVE_SCRIPT.slice(7), false),
    mk(`login-${stamp(t1)}`, 'login', 'https://shop.example.test/', t1, LIVE_SCRIPT.slice(0, 6), true),
  ];
}

const HELPERS = `// ---- helpers (generated; edit freely) -------------------------------------------

type Query = { role?: string; name?: string; text?: string };

/** The ref of the nth (0-based) element matching \`query\`; waits up to 10 s for it. */
async function locate(query: Query, nth = 0): Promise<string> {
  const until = Date.now() + 10_000;
  for (;;) {
    const found = (await browser.page.find({ ...query, limit: nth + 1 })) as { text: string };
    const refs = found.text.match(/@\\d+:e\\d+/g) ?? [];
    if (refs.length > nth) return refs[nth];
    if (Date.now() > until) throw new Error(\`element not found: \${JSON.stringify(query)}\`);
    await sleep(250);
  }
}

/** Act on the element with this role and accessible name. */
async function act(role: string, name: string, action: string, value?: string, nth = 0) {
  const ref = await locate({ role, name }, nth);
  await browser.page.act({ ref, action, value, returns: "none" });
}

/** A required input. Secrets are never written into scripts. */
function need(key: string): string {
  const value = inputs[key];
  if (value === undefined || value === "") throw new Error(\`missing input: --input \${key}=...\`);
  return value;
}

// ---- steps -------------------------------------------------------------------------
`;

const camel = (s: string) =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9]+(.)/g, (_, c: string) => c.toUpperCase())
    .replace(/[^A-Za-z0-9]/g, '') || 'value';

/** A small stand-in for crates/record/src/codegen.rs. */
function codegen(rec: Recording, name: string, parametrize: boolean): { code: string; inputs: CodegenInput[] } {
  const inputs: CodegenInput[] = [];
  const lines: string[] = [];
  const js = (v: string) => JSON.stringify(v);
  const input = (label: string, value: string | null, secret: boolean) => {
    // Same field + same value reuses its input (like the real codegen).
    const same = inputs.find((i) => i.label === label && i.default === value && i.secret === secret);
    if (same) return same.name;
    let n = secret ? 'password' : camel(label);
    while (inputs.some((i) => i.name === n)) n += '2';
    inputs.push({ name: n, label, default: value, secret });
    return n;
  };
  for (const s of rec.steps) {
    const t = s.target;
    const role = t?.role ?? 'generic';
    const tname = t?.name ?? t?.text ?? '';
    const nth = t?.nth ? `, ${t.nth}` : '';
    switch (s.action) {
      case 'start':
        lines.push(`await browser.nav.goto({ url: ${js(s.url ?? '')} });`);
        break;
      case 'navigated':
        lines.push(`await browser.nav.wait({ for: "load" }); // → ${s.url}`);
        break;
      case 'click':
        lines.push(`await act(${js(role)}, ${js(tname)}, "click"${nth ? `, undefined${nth}` : ''});`);
        break;
      case 'fill':
      case 'select': {
        let expr: string;
        if (s.secret) expr = `need(${js(input(tname, null, true))})`;
        else if (parametrize) expr = `inputs.${input(tname, s.value ?? '', false)} ?? ${js(s.value ?? '')}`;
        else expr = js(s.value ?? '');
        lines.push(`await act(${js(role)}, ${js(tname)}, ${js(s.action)}, ${expr});`);
        break;
      }
      case 'press':
        lines.push(`await act(${js(role)}, ${js(tname)}, "press", ${js(s.value ?? '')});`);
        break;
      default:
        break;
    }
  }
  const header = [
    `// Generated by BrowserMCP from recording "${rec.meta.name}" (${rec.meta.id}).`,
    `// Recorded on ${rec.meta.start_url}`,
    '// Types for editors: crates/script/types/browser.d.ts',
    '//',
    inputs.length ? '// Inputs (--input key=value):' : '// Inputs: none.',
    ...inputs.map((i) => `//   ${i.name.padEnd(10)}  ${i.label}${i.secret ? ' (required, secret)' : ''}`),
    '//',
    `// Run: bmcp-script-host run ${name}.ts --ws ws://127.0.0.1:47800/v1/ws`,
    '',
  ].join('\n');
  return { code: `${header}\n${HELPERS}\n${lines.join('\n')}\n`, inputs };
}

const PRICE_WATCH = `// Price watch: reads one product page per row and reports its price.
// Inputs: url (required), sku

await browser.nav.goto({ url: inputs.url });
await browser.nav.wait({ for: "load" });
const price = await browser.page.find({ role: "status", name: "Price" });
browser.output({ sku: inputs.sku ?? "", price: price.text.trim() });
`;

function counts(rows: RunRow[]): RunInfo['counts'] {
  const c = { ok: 0, failed: 0, needs_heal: 0, cancelled: 0, running: 0, pending: 0 };
  for (const r of rows) c[r.status]++;
  return c;
}

function makeRows(n: number, script: string, data?: Record<string, unknown>[]): RunRow[] {
  return Array.from({ length: n }, (_, i) => ({
    row: i,
    status: 'pending' as RowStatus,
    attempts: 0,
    inputs: data?.[i] ?? { url: `https://shop.example.test/p/${1000 + i}`, sku: `SKU-${1000 + i}` },
    result: null,
    error: null,
    script,
  }));
}

export function createRecordRunsMocks(emit: Emit, activeTab: () => TabId | null): RecordRunsHandlers {
  const now = Date.now();
  const recordings = seedRecordings(now);
  let active: { rec: Recording; timer: ReturnType<typeof setInterval>; next: number } | null = null;
  const scripts = new Map<string, { code: string; modified_ms: number }>([
    ['price-watch', { code: PRICE_WATCH, modified_ms: now - 50 * MIN }],
    ['login', { code: codegen(recordings[1], 'login', true).code, modified_ms: now - 25 * 60 * MIN }],
  ]);
  const runs = new Map<string, MockRun>();
  const triggers = new Map<string, Omit<TriggerInfo, 'next_fire' | 'next_fire_ms'>>();

  const options = (parallel: number): RunInfo['options'] => ({
    heal: true,
    parallel: Math.min(parallel, 4),
    parallel_requested: parallel,
    retries: 1,
    stop_on_error: false,
    timeout_secs: 300,
  });
  const refresh = (r: MockRun) => {
    r.info.counts = counts(r.rows);
    r.info.done = r.info.counts.ok + r.info.counts.failed + r.info.counts.cancelled;
  };

  // A finished run with some failures, and a paused one waiting for a patch.
  {
    const rows = makeRows(240, 'price-watch');
    rows.forEach((row, i) => {
      const bad = i % 37 === 5;
      row.status = bad ? 'failed' : 'ok';
      row.attempts = bad ? 2 : 1;
      row.duration_ms = 900 + ((i * 137) % 1600);
      row.result = bad ? null : { sku: row.inputs.sku, price: `$${(899 + ((i * 53) % 700)).toFixed(2)}` };
      row.error = bad ? { code: 'TIMEOUT', command: 'nav.wait', message: 'load did not finish within 30000 ms', line: 5 } : null;
      row.stdout_tail = bad ? '' : `price ${row.result && (row.result as { price: string }).price}\n`;
    });
    const info: RunInfo = {
      id: `run-${now - 90 * MIN}`,
      script: 'price-watch',
      status: 'finished',
      total: rows.length,
      done: 0,
      counts: counts(rows),
      created_ms: now - 90 * MIN,
      finished_ms: now - 84 * MIN,
      options: options(4),
    };
    const r = { info, rows, breakAt: null };
    refresh(r);
    runs.set(info.id, r);
  }
  {
    const rows = makeRows(60, 'login', Array.from({ length: 60 }, (_, i) => ({ email: `user${i}@example.test`, password: '***' })));
    rows.slice(0, 17).forEach((row, i) => {
      row.status = 'ok';
      row.attempts = 1;
      row.duration_ms = 2400 + i * 31;
      row.result = { signedIn: true };
    });
    const heal: HealRequest = {
      id: `run-${now - 12 * MIN}`,
      row: 17,
      script: 'login',
      error: { code: 'NOT_FOUND', command: 'page.find', message: 'element not found: {"role":"button","name":"Sign in"} #0', line: 41 },
      step: 'page.find',
      line: 41,
    };
    rows[17].status = 'needs_heal';
    rows[17].attempts = 1;
    rows[17].error = heal.error;
    const info: RunInfo = {
      id: heal.id,
      script: 'login',
      status: 'paused',
      total: rows.length,
      done: 0,
      counts: counts(rows),
      created_ms: now - 12 * MIN,
      options: options(2),
      heal,
      note: 'waiting for a patched script: call run_heal_apply (or run_resume to skip)',
    };
    const r = { info, rows, breakAt: null };
    refresh(r);
    runs.set(info.id, r);
  }

  triggers.set('trg-nightly', {
    id: 'trg-nightly',
    kind: 'cron',
    spec: '0 2 * * *',
    script: 'price-watch',
    inputs: { url: 'https://shop.example.test/p/1000' },
    created_ms: now - 3 * 24 * 60 * MIN,
    last_fired_ms: now - 9 * 60 * MIN,
    last_status: 'ok',
  });

  const withNext = (t: Omit<TriggerInfo, 'next_fire' | 'next_fire_ms'>): TriggerInfo => {
    let next: Date | undefined;
    try {
      next = nextFires(parseCron(t.spec), new Date(), 1)[0];
    } catch {
      next = undefined;
    }
    return { ...t, next_fire_ms: next?.getTime() ?? null, next_fire: next?.toISOString() ?? null };
  };

  const getRun = (id: string): MockRun => {
    const r = runs.get(id);
    if (!r) throw new RpcError('NOT_FOUND', `no run '${id}'`);
    return r;
  };
  const snapshot = (r: MockRun): RunInfo => {
    refresh(r);
    return structuredClone(r.info);
  };
  const progress = (r: MockRun) => {
    refresh(r);
    emit('run.progress', { id: r.info.id, done: r.info.done, total: r.info.total, ...r.info.counts });
  };
  const finish = (r: MockRun) => {
    refresh(r);
    if (r.info.status === 'running') r.info.status = 'finished';
    r.info.finished_ms = Date.now();
    const c = r.info.counts;
    emit('run.finished', {
      id: r.info.id,
      status: r.info.status,
      total: r.info.total,
      ok: c.ok,
      failed: c.failed,
      cancelled: c.cancelled,
      needs_heal: c.needs_heal,
      duration_ms: Date.now() - r.info.created_ms,
    });
  };

  // Runs make progress: running rows end, free slots take pending rows. The timer only runs
  // while some run does (MockApi instances in tests must not keep a timer alive).
  let ticker: ReturnType<typeof setInterval> | null = null;
  const tick = () => {
    if (![...runs.values()].some((r) => r.info.status === 'running')) {
      if (ticker) clearInterval(ticker);
      ticker = null;
      return;
    }
    for (const r of runs.values()) {
      if (r.info.status !== 'running') continue;
      let changed = false;
      for (const row of r.rows.filter((x) => x.status === 'running')) {
        if (Math.random() < 0.5) continue;
        changed = true;
        row.duration_ms = 600 + Math.round(Math.random() * 2000);
        if (r.breakAt === row.row) {
          row.status = 'needs_heal';
          row.error = { code: 'NOT_FOUND', command: 'page.find', message: 'element not found: {"role":"status","name":"Price"} #0', line: 6 };
          progress(r);
          const heal: HealRequest = { id: r.info.id, row: row.row, script: r.info.script, error: row.error, step: 'page.find', line: 6 };
          r.info.heal = heal;
          r.info.status = 'paused';
          r.info.note = 'waiting for a patched script: call run_heal_apply (or run_resume to skip)';
          emit('run.heal_needed', heal);
          break;
        }
        const fail = Math.random() < 0.08;
        row.attempts = fail ? 1 + r.info.options.retries : 1;
        row.status = fail ? 'failed' : 'ok';
        row.error = fail ? { code: 'SCRIPT_ERROR', command: null, message: 'TypeError: price.text is undefined', line: 7 } : null;
        row.result = fail ? null : { sku: row.inputs.sku ?? row.row, price: `$${(500 + Math.random() * 900).toFixed(2)}` };
        row.stdout_tail = fail ? 'TypeError: price.text is undefined\n    at main (price-watch.ts:7:22)' : '';
        if (fail) emit('run.row_failed', { id: r.info.id, row: row.row, error: row.error, attempts: row.attempts });
      }
      if (r.info.status !== 'running') continue;
      let running = r.rows.filter((x) => x.status === 'running').length;
      for (const row of r.rows) {
        if (running >= r.info.options.parallel) break;
        if (row.status !== 'pending') continue;
        row.status = 'running';
        row.attempts = 1;
        running++;
        changed = true;
      }
      if (changed) progress(r);
      if (!r.rows.some((x) => x.status === 'pending' || x.status === 'running' || x.status === 'needs_heal')) finish(r);
    }
  };
  const wake = () => {
    ticker ??= setInterval(tick, 700);
  };

  const startRecording = (tab: TabId, name: string, video: boolean, traffic: boolean): Recording => {
    const started = Date.now();
    const id = `${name}-${stamp(started)}`;
    const startUrl = 'https://shop.example.test/';
    const rec: Recording = {
      meta: {
        id,
        name,
        tab,
        started_ms: started,
        start_url: startUrl,
        steps: 1,
        human_capture: 'binding',
        video: video ? `${DATA}\\recordings\\${id}\\video.webm` : null,
        traffic: traffic ? `${DATA}\\recordings\\${id}\\traffic.jsonl` : null,
      },
      steps: [{ seq: 1, ts_ms: started, source: 'system', action: 'start', url: startUrl }],
    };
    recordings.unshift(rec);
    let i = 0;
    const timer = setInterval(() => {
      const tpl = LIVE_SCRIPT[i % LIVE_SCRIPT.length];
      i++;
      const step: RecordStep = structuredClone({ ...tpl, seq: rec.steps.length + 1, ts_ms: Date.now() });
      if (step.action === 'navigated' || step.action === 'goto') step.url = tpl.url;
      else step.url = 'https://shop.example.test/';
      // API steps arrive twice: pending first, then with their result.
      if (step.command) {
        const pending = structuredClone(step);
        delete pending.command!.ok;
        emit('record.step', { id, step: pending });
        setTimeout(() => emit('record.step', { id, step }), 300);
      } else {
        emit('record.step', { id, step });
      }
      rec.steps.push(step);
      rec.meta.steps = rec.steps.length;
    }, 1400);
    active = { rec, timer, next: 0 };
    emit('record.started', { id, tab, name, start_url: startUrl });
    emit('record.step', { id, step: rec.steps[0] });
    return rec;
  };

  const findRec = (id: string): Recording => {
    const rec = recordings.find((r) => r.meta.id === id);
    if (!rec) throw new RpcError('NOT_FOUND', `no recording "${id}"`);
    return rec;
  };

  return {
    'record.start': ({ tab, name, video, traffic }) => {
      if (active) throw new RpcError('INVALID_PARAMS', `already recording (${active.rec.meta.id})`);
      const t = tab ?? activeTab();
      if (t === null || t === undefined) throw new RpcError('NOT_FOUND', 'no tab to record');
      const rec = startRecording(t, (name ?? 'recording').replace(/[^A-Za-z0-9_-]+/g, '-') || 'recording', !!video, !!traffic);
      return { id: rec.meta.id, tab: t, dir: `${DATA}\\recordings\\${rec.meta.id}`, video: rec.meta.video };
    },
    'record.stop': ({ id }) => {
      if (!active || (id && active.rec.meta.id !== id)) throw new RpcError('NOT_FOUND', 'no running recording');
      const { rec, timer } = active;
      clearInterval(timer);
      active = null;
      rec.meta.stopped_ms = Date.now();
      emit('record.stopped', { id: rec.meta.id, steps: rec.steps.length, video: rec.meta.video ?? null });
      const dir = `${DATA}\\recordings\\${rec.meta.id}`;
      return {
        id: rec.meta.id,
        steps: rec.steps.length,
        human_capture: 'binding',
        actions: `${dir}\\actions.jsonl`,
        meta: `${dir}\\meta.json`,
        video: rec.meta.video,
        traffic: rec.meta.traffic,
      };
    },
    'record.list': () => ({
      recordings: recordings.map((r) => ({
        id: r.meta.id,
        name: r.meta.name,
        active: active?.rec === r,
        steps: r.steps.length,
        started_ms: r.meta.started_ms,
        start_url: r.meta.start_url,
      })),
    }),
    'record.get': ({ id, format }) => {
      const rec = findRec(id);
      const text =
        format === 'jsonl'
          ? rec.steps.map((s) => JSON.stringify(s)).join('\n') + '\n'
          : rec.steps.map((s) => `#${s.seq} ${s.source} ${s.action}`).join('\n');
      return { id, active: active?.rec === rec, steps: rec.steps.length, meta: { ...rec.meta, steps: rec.steps.length }, text };
    },
    'record.codegen': async ({ id, name, parametrize }) => {
      await sleep(250);
      const rec = findRec(id);
      const n = name ?? rec.meta.name;
      const out = codegen(rec, n, !!parametrize);
      scripts.set(n, { code: out.code, modified_ms: Date.now() });
      return { path: `${DATA}\\scripts\\${n}.ts`, ...out };
    },
    'record.video_start': ({ tab }) => {
      const t = tab ?? activeTab() ?? 1;
      return { id: `v${Date.now()}`, tab: t, path: `${DATA}\\videos\\tab${t}-${stamp(Date.now())}.webm` };
    },
    'record.video_stop': () => ({
      path: `${DATA}\\videos\\last.webm`,
      bytes: 1_840_000,
      codec: 'vp9',
      fps: 10,
      frames_received: 233,
      frames_written: 240,
      width: 1280,
      height: 720,
      seconds: 24,
      wall_seconds: 24.1,
    }),
    'script.list': () => ({
      scripts: [...scripts.entries()]
        .map(([name, s]) => ({ name, size: new Blob([s.code]).size, modified_ms: s.modified_ms }))
        .sort((a, b) => a.name.localeCompare(b.name)),
    }),
    'script.get': ({ name }) => {
      const s = scripts.get(name);
      if (!s) throw new RpcError('NOT_FOUND', `no script '${name}'`);
      return { name, code: s.code };
    },
    'script.save': ({ name, code }) => {
      if (!/^[A-Za-z0-9_-]{1,64}$/.test(name)) throw new RpcError('INVALID_PARAMS', 'name must be 1-64 chars of A-Z a-z 0-9 _ -');
      const modified_ms = Date.now();
      scripts.set(name, { code, modified_ms });
      return { name, size: new Blob([code]).size, modified_ms };
    },
    'script.delete': ({ name }) => ({ deleted: scripts.delete(name) }),
    'script.run': async ({ name, code, inputs }) => {
      const src = code ?? (name ? scripts.get(name)?.code : undefined);
      if (src === undefined) throw new RpcError('NOT_FOUND', `no script '${name}'`);
      await sleep(1200);
      const missing = [...src.matchAll(/need\("([^"]+)"\)/g)].map((m) => m[1]).find((k) => !inputs?.[k]);
      if (missing) {
        return {
          ok: false,
          exit_code: 1,
          duration_ms: 1180,
          result: null,
          stdout: '',
          stderr: `Error: missing input: --input ${missing}=...\n    at need (${name}.ts:38:9)`,
          error: { code: 'SCRIPT_ERROR', command: null, message: `missing input: --input ${missing}=...`, line: 38 },
        };
      }
      return {
        ok: true,
        exit_code: 0,
        duration_ms: 1184,
        result: { signedIn: true, inputs: Object.keys(inputs ?? {}) },
        stdout: 'navigated to https://shop.example.test/account\n',
        stderr: '',
        error: null,
      };
    },
    'run.start': ({ script, data, parallel, retries, timeout_secs, heal, stop_on_error }) => {
      if (!scripts.has(script)) throw new RpcError('NOT_FOUND', `no script '${script}'`, { hint: 'script_list shows the saved scripts' });
      let rows: RunRow[];
      if (data.rows) rows = makeRows(data.rows.length, script, data.rows);
      else if (data.csv_path || data.json_path || data.xlsx_path) rows = makeRows(48, script);
      else throw new RpcError('INVALID_PARAMS', 'data needs one of csv_path, json_path, xlsx_path or rows');
      if (rows.length === 0) throw new RpcError('INVALID_PARAMS', 'the data has no rows');
      const created = Date.now();
      const info: RunInfo = {
        id: `run-${created}`,
        script,
        status: 'running',
        total: rows.length,
        done: 0,
        counts: counts(rows),
        created_ms: created,
        options: { ...options(parallel ?? 4), retries: retries ?? 1, timeout_secs: timeout_secs ?? 300, heal: heal === undefined || heal === null ? true : heal !== false && heal !== 'off', stop_on_error: !!stop_on_error },
      };
      // Heal demo: with healing on, the row in the middle breaks on a selector.
      const r: MockRun = { info, rows, breakAt: (heal ?? true) && rows.length >= 6 ? Math.floor(rows.length / 2) : null };
      runs.set(info.id, r);
      wake();
      emit('run.started', { id: info.id, script, total: rows.length, parallel: info.options.parallel });
      return snapshot(r);
    },
    'run.status': ({ id }) => snapshot(getRun(id)),
    'run.list': ({ limit }) => ({
      runs: [...runs.values()]
        .map(snapshot)
        .sort((a, b) => b.created_ms - a.created_ms)
        .slice(0, limit ?? 50),
    }),
    'run.results': ({ id, cursor, limit, status }) => {
      const r = getRun(id);
      const n = Math.min(limit ?? 100, 1000);
      const from = cursor ?? 0;
      const matching = r.rows.filter((x) => x.row >= from && (!status || x.status === status));
      const page = matching.slice(0, n);
      const more = matching.length > n;
      return { id, rows: structuredClone(page), next_cursor: more ? page[page.length - 1].row + 1 : null };
    },
    'run.pause': ({ id }) => {
      const r = getRun(id);
      if (r.info.status === 'running') {
        r.info.status = 'paused';
        r.info.note = 'paused by run_pause';
        for (const row of r.rows) if (row.status === 'running') row.status = 'pending';
      } else if (r.info.status !== 'paused') throw new RpcError('INVALID_PARAMS', `run '${id}' is ${r.info.status}`);
      return snapshot(r);
    },
    'run.resume': ({ id }) => {
      const r = getRun(id);
      if (r.info.status !== 'paused' && r.info.status !== 'running') throw new RpcError('INVALID_PARAMS', `run '${id}' is ${r.info.status}`);
      for (const row of r.rows) if (row.status === 'needs_heal') row.status = 'failed';
      r.info.heal = null;
      r.info.note = null;
      r.info.status = 'running';
      r.breakAt = null;
      wake();
      return snapshot(r);
    },
    'run.cancel': ({ id }) => {
      const r = getRun(id);
      if (r.info.status === 'running' || r.info.status === 'paused') {
        for (const row of r.rows) if (row.status === 'pending' || row.status === 'needs_heal' || row.status === 'running') row.status = 'cancelled';
        r.info.status = 'cancelled';
        r.info.heal = null;
        r.info.note = 'cancelled by run_cancel';
        finish(r);
      }
      return snapshot(r);
    },
    'run.export': async ({ id, to, url, status }) => {
      const r = getRun(id);
      await sleep(300);
      const rows = r.rows.filter((x) => !status || x.status === status).length;
      if (to === 'google_sheets') throw new RpcError('NOT_IMPLEMENTED', 'google_sheets export is not available yet (needs Google OAuth)');
      if (to === 'webhook') {
        if (!url || !/^https?:\/\//.test(url)) throw new RpcError('INVALID_PARAMS', 'webhook export needs an http(s) url');
        return { to, rows, batches: Math.ceil(rows / 100) };
      }
      return { to, rows, path: `${DATA}\\runs\\${id}\\results.${to}` };
    },
    'run.heal_apply': ({ id, patched_code }) => {
      const r = getRun(id);
      if (r.info.status !== 'paused' && r.info.status !== 'running') throw new RpcError('INVALID_PARAMS', `run '${id}' is ${r.info.status}`);
      const base = r.info.script.replace(/\.v\d+$/, '');
      let v = 2;
      while (scripts.has(`${base}.v${v}`)) v++;
      const script = `${base}.v${v}`;
      scripts.set(script, { code: patched_code, modified_ms: Date.now() });
      let requeued = 0;
      for (const row of r.rows) {
        if (row.status === 'needs_heal') {
          row.status = 'pending';
          row.error = null;
          requeued++;
        }
      }
      r.info.script = script;
      r.info.heal = null;
      r.info.note = null;
      r.info.status = 'running';
      r.breakAt = null;
      wake();
      return { id, script, requeued };
    },
    'trigger.set': ({ id, kind, spec, script, inputs }) => {
      if (kind !== 'cron') throw new RpcError('INVALID_PARAMS', 'only cron triggers');
      try {
        parseCron(spec);
      } catch (err) {
        throw new RpcError('INVALID_PARAMS', `invalid cron spec ${JSON.stringify(spec)}: ${(err as Error).message}`);
      }
      if (!scripts.has(script)) throw new RpcError('NOT_FOUND', `no script '${script}'`);
      const t = {
        id: id ?? `trg-${Date.now()}`,
        kind: 'cron',
        spec: spec.trim(),
        script,
        inputs: inputs ?? {},
        created_ms: Date.now(),
        last_fired_ms: null,
        last_status: null,
      };
      triggers.set(t.id, t);
      return withNext(t);
    },
    'trigger.list': () => ({ triggers: [...triggers.values()].map(withNext) }),
    'trigger.remove': ({ id }) => ({ removed: triggers.delete(id) }),
  };
}
