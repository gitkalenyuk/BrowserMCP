// Fakes for the settings page and the heal UI (notify.*, run.heal_*, governor.*, hints.config,
// adblock.*, simplify.*, plugins.*, userscripts.*), used by MockApi so `pnpm dev` demos every
// section without a backend. Shapes follow the generated protocol types; behaviour is only
// plausible (the crates are the truth). No network: Telegram / webhook tests just check the
// form like the core does before it would call out.

import { RpcError } from './client';
import type { ApiEvents, EventName, Method, Params, Result } from './types';
import type { RunInfo } from './types-record-runs';
import type {
  AdblockSettings,
  GovernorLimits,
  HealProposal,
  HintsConfig,
  NotifyConfig,
  PluginInfo,
  PluginLogLine,
  SettingsMethods,
  SitePolicy,
  UserscriptInfo,
} from './types-settings';
import type { E2SettingsMethods } from './types-e2-settings';
import { DEFAULT_RULES } from '../settings/notify';

type Emit = <E extends EventName>(event: E, payload: ApiEvents[E]) => void;
type On = <E extends EventName>(event: E, handler: (payload: ApiEvents[E]) => void) => () => void;
type Call = <M extends Method>(method: M, params: Params<M>) => Promise<Result<M>>;

export type SettingsHandlers = {
  [M in keyof SettingsMethods]: (params: Params<M>) => Result<M> | Promise<Result<M>>;
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const DATA = 'C:\\Users\\you\\AppData\\Roaming\\BrowserMCP\\data';

const PROFILES: Record<GovernorLimits['profile'], Pick<GovernorLimits, 'freeze_after_secs' | 'discard_after_secs' | 'max_live_tabs'>> = {
  low: { freeze_after_secs: 120, discard_after_secs: 600, max_live_tabs: 8 },
  normal: { freeze_after_secs: 300, discard_after_secs: 1800, max_live_tabs: 20 },
  high: { freeze_after_secs: 600, discard_after_secs: 3600, max_live_tabs: 40 },
};

function mask(token: string): string {
  return token ? `…${token.slice(-4)}` : '';
}

/** A one-line fix for the failing line, as a unified diff (3 lines of context). */
export function mockPatch(code: string, line: number | null, name: string): { diff: string; patched: string; explanation: string } {
  const lines = code.split('\n');
  const failing = Math.min(Math.max((line ?? 1) - 1, 0), Math.max(lines.length - 1, 0));
  // The nearest line with a locator (the failing line itself first).
  const near = [...lines.keys()]
    .filter((i) => Math.abs(i - failing) <= 6 && /(act\(|name:)\s*.*"[^"]+"/.test(lines[i]))
    .sort((a, b) => Math.abs(a - failing) - Math.abs(b - failing) || b - a);
  const at = near[0] ?? failing;
  const old = lines[at] ?? '';
  let fixed = old;
  let explanation = 'The failing call had no locator to fix; marked it for a retry.';
  if (old.includes('"Sign in"')) {
    fixed = old.replace('"Sign in"', '"Log in"');
    explanation = 'The page now labels the button “Log in” instead of “Sign in”; the snapshot shows no other sign-in control.';
  } else if (/name: "([^"]+)"/.test(old)) {
    fixed = old.replace(/name: "([^"]+)"/, (_, n: string) => `name: /${n.replace(/[^\w ]/g, '')}/i`);
    explanation = 'The element is still there but its accessible name changed case; match it case-insensitively.';
  }
  if (fixed === old) fixed = `${old} // retried by the AI`;
  const patched = [...lines.slice(0, at), fixed, ...lines.slice(at + 1)].join('\n');
  const from = Math.max(0, at - 3);
  const to = Math.min(lines.length, at + 4);
  const body: string[] = [];
  for (let i = from; i < to; i++) {
    if (i === at) body.push(`-${old}`, `+${fixed}`);
    else body.push(` ${lines[i]}`);
  }
  const n = to - from;
  const diff = [`--- a/${name}.ts`, `+++ b/${name}.ts`, `@@ -${from + 1},${n} +${from + 1},${n} @@`, ...body].join('\n');
  return { diff, patched, explanation };
}

function headerOf(code: string): UserscriptInfo['header'] | null {
  const tag = (t: string) => [...code.matchAll(new RegExp(`^//\\s*@${t}\\s+(.+)$`, 'gm'))].map((m) => m[1].trim());
  const matches = tag('match');
  const includes = tag('include');
  if (!matches.length && !includes.length) return null;
  const runAt = tag('run-at')[0] as 'document-start' | 'document-end' | 'document-idle' | undefined;
  return {
    name: tag('name')[0] ?? null,
    description: tag('description')[0] ?? null,
    version: tag('version')[0] ?? null,
    matches,
    includes,
    excludes: tag('exclude'),
    exclude_matches: tag('exclude-match'),
    run_at: runAt ?? 'document-idle',
  };
}

export function createSettingsMocks(emit: Emit, on: On, call: Call): SettingsHandlers & E2SettingsHandlers {
  const now = Date.now();

  // ---- notify ----
  let notify: NotifyConfig = {
    channels: {
      ui: true,
      os: true,
      telegram: { enabled: false, bot_token: '', chat_id: '', api_base: 'https://api.telegram.org' },
      webhook: { enabled: false, url: '' },
    },
    rules: structuredClone(DEFAULT_RULES),
    limits: { dedup_secs: 10, aggregate_secs: 60, max_per_minute: 20 },
  };
  let realToken = '';
  const maskedNotify = (): NotifyConfig => {
    const c = structuredClone(notify);
    c.channels.telegram.bot_token = mask(realToken);
    return c;
  };

  // ---- governor ----
  let overrides: Partial<GovernorLimits> = {};
  const limits = (): GovernorLimits => {
    const profile = overrides.profile ?? 'normal';
    return {
      profile,
      enabled: true,
      busy_secs: 30,
      pressure_free_pct: 15,
      tick_secs: 15,
      ...PROFILES[profile],
      ...overrides,
    };
  };

  // ---- hints ----
  let hints: HintsConfig = {
    enabled: true,
    alphabet: 'asdfghjkl',
    keys: { click: 'f', newtab: 'F', focus: null, copy: null },
    disabled_sites: ['mail.google.com'],
  };

  // ---- adblock / simplify ----
  let adblock: AdblockSettings = { enabled: true, allowlist: ['example.org'], lists: ['easylist', 'easyprivacy'] };
  const sites: Record<string, SitePolicy> = {
    '*': { animations: true },
    'news.example.com': { js: false, images: true, third_party: false },
    'heavy.example.net': { media: false, fonts: false },
  };
  const effective = (host: string) => {
    const base = { js: true, images: true, fonts: true, media: true, third_party: true, animations: true };
    const matched = Object.keys(sites)
      .filter((p) => p === '*' || host === p || host.endsWith(`.${p}`))
      .sort((a, b) => (a === '*' ? -1 : b === '*' ? 1 : a.length - b.length));
    for (const p of matched) for (const [k, v] of Object.entries(sites[p])) if (v !== null && v !== undefined) base[k as keyof typeof base] = v;
    return { matched, policy: base };
  };
  let blocked = 1240;

  // ---- plugins / userscripts ----
  const plugins: PluginInfo[] = [
    {
      name: 'reading-time',
      dir: `${DATA}\\plugins\\reading-time`,
      enabled: true,
      status: 'running',
      pid: 18244,
      restarts: 0,
      manifest: {
        name: 'reading-time',
        version: '1.2.0',
        description: 'Shows how long an article takes to read and adds a Ctrl+K entry to copy it.',
        main: 'main.ts',
        permissions: ['page.read', 'tabs.list', 'ui.notify'],
        events: ['tab.updated'],
        ui: {
          palette: [
            { id: 'estimate', title: 'Reading time of this page' },
            { id: 'copy', title: 'Copy reading time' },
          ],
        },
        userscripts: ['badge.ts'],
      },
      warnings: [],
    },
    {
      name: 'price-alerts',
      dir: `${DATA}\\plugins\\price-alerts`,
      enabled: false,
      status: 'disabled',
      pid: null,
      restarts: 0,
      manifest: {
        name: 'price-alerts',
        version: '0.3.1',
        description: 'Checks saved product pages every hour and notifies on price drops.',
        main: 'main.ts',
        permissions: ['*'],
        events: ['trigger.fired'],
        ui: { palette: [{ id: 'watch', title: 'Watch the price of this page' }] },
        userscripts: [],
      },
      warnings: ['"*" grants every command, including passwords.* and term.*'],
    },
    {
      name: 'broken-thing',
      dir: `${DATA}\\plugins\\broken-thing`,
      enabled: true,
      status: 'crashed',
      pid: null,
      restarts: 5,
      error: "TypeError: Cannot read properties of undefined (reading 'tab') at main.ts:12",
      manifest: null,
      warnings: [],
    },
  ];
  const logs = new Map<string, PluginLogLine[]>([
    [
      'reading-time',
      [
        { ts_ms: now - 60_000, stream: 'system', line: 'started (pid 18244)' },
        { ts_ms: now - 55_000, stream: 'stdout', line: 'listening for tab.updated' },
        { ts_ms: now - 20_000, stream: 'stdout', line: 'tab 3: 1840 words → 8 min' },
      ],
    ],
    [
      'broken-thing',
      [
        { ts_ms: now - 90_000, stream: 'system', line: 'started (pid 9120)' },
        { ts_ms: now - 89_000, stream: 'stderr', line: "TypeError: Cannot read properties of undefined (reading 'tab')" },
        { ts_ms: now - 88_000, stream: 'system', line: 'exited with code 1; restarting in 2 s' },
        { ts_ms: now - 30_000, stream: 'system', line: 'crashed 5 times in a row; giving up' },
      ],
    ],
  ]);
  const log = (name: string, stream: string, line: string) => logs.set(name, [...(logs.get(name) ?? []), { ts_ms: Date.now(), stream, line }].slice(-500));
  const plugin = (name: string) => {
    const p = plugins.find((x) => x.name === name);
    if (!p) throw new RpcError('NOT_FOUND', `plugin ${name} is not installed`);
    return p;
  };

  const userCode = new Map<string, string>([
    [
      'dark-docs',
      `// @name Dark docs\n// @description Darker code blocks on docs.rs\n// @match https://docs.rs/*\n// @run-at document-end\n\nawait browser.userscripts.inject({\n  tab: inputs.tab,\n  js: "document.documentElement.style.colorScheme = 'dark'",\n});\n`,
    ],
    ['hide-cookie-banners', `// @match https://*/*\n// @exclude-match https://bank.example.com/*\n// @run-at document-idle\n\nawait browser.userscripts.inject({ tab: inputs.tab, js: "document.querySelectorAll('[id*=cookie]').forEach(e => e.remove())" });\n`],
  ]);
  const disabledUser = new Set<string>(['hide-cookie-banners']);
  const userInfo = (name: string): UserscriptInfo => {
    const code = userCode.get(name)!;
    const header = headerOf(code);
    return {
      name,
      source: 'user',
      enabled: !disabledUser.has(name),
      path: `${DATA}\\userscripts\\${name}.ts`,
      ...(header ? { header } : { error: 'no // @match or // @include line' }),
      last_run:
        name === 'dark-docs'
          ? { at_ms: now - 4 * 60_000, duration_ms: 212, ok: true, tab: 3, url: 'https://docs.rs/cef/latest/cef/' }
          : null,
    };
  };

  // ---- heal ----
  const proposals = new Map<string, HealProposal>();
  const attempts = new Map<string, number>();
  let nextProposal = 1;

  async function makeProposal(run: RunInfo, feedback?: string): Promise<HealProposal> {
    const heal = run.heal as { row: number; script: string; line: number | null } | null | undefined;
    if (!heal) throw new RpcError('INVALID_PARAMS', `run '${run.id}' is not waiting for a patch`);
    const attempt = (attempts.get(run.id) ?? 0) + 1;
    if (attempt > 3) throw new RpcError('INVALID_PARAMS', 'no attempts left: fix it by hand (run.heal_apply) or skip the row (run.resume)');
    attempts.set(run.id, attempt);
    await sleep(900);
    let code = '';
    try {
      code = (await call('script.get', { name: heal.script })).code;
    } catch {
      code = '// script not found\n';
    }
    const fix = mockPatch(code, heal.line, heal.script);
    for (const p of proposals.values()) if (p.id === run.id && p.status === 'pending') p.status = 'superseded';
    const p: HealProposal = {
      proposal_id: `hp${nextProposal++}`,
      id: run.id,
      row: heal.row,
      script: heal.script,
      diff: fix.diff,
      patched_code: fix.patched,
      explanation: feedback ? `${fix.explanation} (Reviewer: “${feedback}”.)` : fix.explanation,
      confidence: Math.max(0.35, 0.86 - (attempt - 1) * 0.17),
      attempt,
      max_attempts: 3,
      checks: ['applies', `region ${Math.max(1, (heal.line ?? 1) - 15)}-${(heal.line ?? 1) + 15}`, 'transpiles'],
      status: 'pending',
      provider: 'claude-code',
      model: null,
      created_ms: Date.now(),
    };
    proposals.set(p.proposal_id, p);
    const mode = run.options.heal_mode ?? 'ask';
    const { provider: _p, model: _m, created_ms: _c, status: _s, ...event } = p;
    emit('run.heal_proposed', { ...event, mode });
    return p;
  }

  const runOf = (id: string) => call('run.status', { id });
  /** Like crates/heal: pending proposals that no longer answer the run's open heal request (the
   *  run was cancelled, finished, resumed or fixed by hand) become superseded. */
  async function reconcile(runId: string): Promise<void> {
    let open: { row: number; script: string } | null = null;
    try {
      const run = await runOf(runId);
      const heal = run.heal as { row: number; script: string } | null | undefined;
      if (heal && (run.status === 'paused' || run.status === 'running')) open = heal;
    } catch (err) {
      if (!(err instanceof RpcError) || err.code !== 'NOT_FOUND') return;
    }
    for (const p of proposals.values()) {
      if (p.id === runId && p.status === 'pending' && (open?.row !== p.row || open?.script !== p.script)) p.status = 'superseded';
    }
  }
  const pendingOf = async (proposal_id: string) => {
    const p = proposals.get(proposal_id);
    if (!p) throw new RpcError('NOT_FOUND', `no proposal ${proposal_id} (proposals are kept in memory)`);
    const was = p.status;
    if (was === 'pending') await reconcile(p.id);
    if (was === 'pending' && p.status === 'superseded') throw new RpcError('INVALID_PARAMS', `proposal ${proposal_id} is outdated: run '${p.id}' is no longer waiting for this fix`);
    if (p.status !== 'pending') throw new RpcError('INVALID_PARAMS', `proposal ${proposal_id} is ${p.status}, not pending`);
    return p;
  };

  // The heal watcher: a paused run gets a proposal by itself (unless healing is off).
  on('run.heal_needed', (h) => {
    void (async () => {
      const run = await runOf(h.id);
      if ((run.options.heal_mode ?? 'ask') === 'off') return;
      attempts.delete(h.id);
      await makeProposal(run).catch(() => {});
    })();
  });
  // The seeded paused run of the mock has one waiting already.
  setTimeout(() => {
    void call('run.list', {}).then(async ({ runs }) => {
      for (const r of runs) if (r.heal && ![...proposals.values()].some((p) => p.id === r.id)) await makeProposal(r).catch(() => {});
    });
  }, 0);

  return {
    ...createE2SettingsMocks(call),
    // ---- notify ----
    'notify.config_get': () => maskedNotify(),
    'notify.config_set': (args) => {
      const next = structuredClone(notify);
      if (args.ui != null) next.channels.ui = args.ui;
      if (args.os != null) next.channels.os = args.os;
      if (args.telegram) {
        const t = args.telegram;
        if (t.enabled != null) next.channels.telegram.enabled = t.enabled;
        if (t.chat_id != null) next.channels.telegram.chat_id = t.chat_id;
        if (t.api_base != null) next.channels.telegram.api_base = t.api_base || 'https://api.telegram.org';
        if (t.bot_token != null && !t.bot_token.startsWith('…')) realToken = t.bot_token;
      }
      if (args.webhook) {
        if (args.webhook.enabled != null) next.channels.webhook.enabled = args.webhook.enabled;
        if (args.webhook.url != null) next.channels.webhook.url = args.webhook.url;
      }
      if (args.rules) {
        for (const r of args.rules) if (!r.topic.trim()) throw new RpcError('INVALID_PARAMS', 'rule topic is empty');
        next.rules = args.rules.map((r) => ({ aggregate: r.aggregate ?? false, when: r.when ?? 'always', channels: r.channels, topic: r.topic }));
      }
      if (args.limits) next.limits = { ...next.limits, ...args.limits };
      notify = next;
      return maskedNotify();
    },
    'notify.test': async ({ channel }) => {
      await sleep(350);
      const c = notify.channels;
      let error: string | null = null;
      if (channel === 'telegram' && (!realToken || !c.telegram.chat_id)) error = 'telegram: bot_token and chat_id are not set';
      else if (channel === 'telegram') error = 'telegram: the mock does not call Telegram (no network in pnpm dev)';
      else if (channel === 'webhook' && !c.webhook.url) error = 'webhook: url is not set';
      else if (channel === 'ui')
        emit('ui.notification', { id: `n${Date.now()}`, title: 'BrowserMCP test', body: 'Notifications to ui work.', level: 'info', actions: [], sticky: false, topic: 'notify.test' });
      return { channel, ok: !error, error };
    },

    // ---- heal ----
    'run.heal_propose': async ({ id, feedback }) => {
      const { patched_code: _code, ...p } = await makeProposal(await runOf(id), feedback ?? undefined);
      return p;
    },
    'run.heal_accept': async ({ proposal_id }) => {
      const p = await pendingOf(proposal_id);
      const r = await call('run.heal_apply', { id: p.id, patched_code: p.patched_code });
      p.status = 'accepted';
      attempts.delete(p.id);
      emit('run.heal_applied', { id: p.id, proposal_id, script: r.script, requeued: r.requeued, auto: false });
      return { id: p.id, proposal_id, script: r.script, requeued: r.requeued };
    },
    'run.heal_reject': async ({ proposal_id, feedback }) => {
      const p = await pendingOf(proposal_id);
      p.status = 'rejected';
      if ((attempts.get(p.id) ?? 0) >= p.max_attempts) {
        const error = `no usable patch after ${p.max_attempts} attempts`;
        emit('run.heal_failed', { id: p.id, row: p.row, attempts: p.max_attempts, error });
        throw new RpcError('INVALID_PARAMS', error);
      }
      const { patched_code: _code, ...next } = await makeProposal(await runOf(p.id), feedback ?? undefined);
      return next;
    },
    'run.heal_proposals': async ({ id }) => {
      const runs = new Set([...proposals.values()].filter((p) => p.status === 'pending' && (!id || p.id === id)).map((p) => p.id));
      for (const run of runs) await reconcile(run);
      return { proposals: [...proposals.values()].filter((p) => !id || p.id === id).sort((a, b) => b.created_ms - a.created_ms) };
    },

    // ---- governor ----
    'governor.config': (args) => {
      if (args.reset) overrides = {};
      for (const k of ['busy_secs', 'discard_after_secs', 'enabled', 'freeze_after_secs', 'max_live_tabs', 'pressure_free_pct', 'profile', 'tick_secs'] as const) {
        const v = args[k];
        if (v !== undefined && v !== null) (overrides as Record<string, unknown>)[k] = v;
      }
      if (args.profile) for (const k of ['freeze_after_secs', 'discard_after_secs', 'max_live_tabs'] as const) if (args[k] == null) delete overrides[k];
      return { limits: limits(), overrides: { ...overrides } };
    },
    'governor.status': async () => {
      const snapshot = await call('tabs.list', undefined as never);
      const l = limits();
      const tabs = snapshot.tabs.map((t, i) => {
        const active = t.id === snapshot.activeId;
        const protectedBy = active ? 'active tab' : t.lockedBy ? `agent ${t.lockedBy}` : t.url.startsWith('bmcp:') ? 'browser page' : null;
        const idle = active ? 0 : 40 + i * 410;
        const state: 'live' | 'frozen' | 'discarded' =
          protectedBy !== null ? 'live' : l.enabled && l.discard_after_secs && idle >= l.discard_after_secs ? 'discarded' : l.enabled && l.freeze_after_secs && idle >= l.freeze_after_secs ? 'frozen' : 'live';
        return {
          id: t.id,
          title: t.title,
          url: t.url,
          state,
          idle_secs: idle,
          js_heap_mb: state === 'live' ? 18 + ((t.id * 37) % 140) : null,
          protected: protectedBy,
          saved: state === 'discarded' ? { url: t.url, title: t.title, scroll_x: 0, scroll_y: 640 } : null,
        };
      });
      return {
        limits: l,
        overrides: { ...overrides },
        memory: {
          total_mb: 16_303,
          available_mb: 7_412,
          free_pct: 45,
          pressure: false,
          browser_mb: 1_284,
          renderers: tabs.filter((t) => t.state !== 'discarded').length,
          processes: [
            { pid: 4412, kind: 'browser', mb: 212 },
            { pid: 9120, kind: 'gpu-process', mb: 188 },
            { pid: 9344, kind: 'utility', mb: 61 },
            ...tabs.filter((t) => t.state !== 'discarded').map((t, i) => ({ pid: 10_000 + i, kind: 'renderer', mb: 80 + (t.js_heap_mb ?? 0) })),
          ],
        },
        decisions: [
          { ts_ms: now - 9 * 60_000, tab: tabs[2]?.id ?? 3, action: 'freeze' as const, changed: true, reason: 'idle 5 min in the background' },
          { ts_ms: now - 2 * 60_000, tab: tabs[2]?.id ?? 3, action: 'thaw' as const, changed: true, reason: 'activated' },
        ],
        tabs,
      };
    },

    // ---- hints ----
    'hints.config': (args) => {
      const next = structuredClone(hints);
      if (args.enabled != null) next.enabled = args.enabled;
      if (args.alphabet != null) {
        if (!/^[a-z0-9]{2,}$/.test(args.alphabet) || new Set(args.alphabet).size !== args.alphabet.length) {
          throw new RpcError('INVALID_PARAMS', 'alphabet: 2+ different lower-case letters/digits');
        }
        next.alphabet = args.alphabet;
      }
      if (args.keys) next.keys = { click: 'f', newtab: 'F', focus: null, copy: null, ...args.keys } as HintsConfig['keys'];
      if (args.disabled_sites) next.disabled_sites = [...args.disabled_sites];
      if (args.disable_site && !next.disabled_sites.includes(args.disable_site)) next.disabled_sites = [...next.disabled_sites, args.disable_site].sort();
      if (args.enable_site) next.disabled_sites = next.disabled_sites.filter((s) => s !== args.enable_site);
      hints = next;
      return { config: structuredClone(hints) };
    },

    // ---- adblock / simplify ----
    'adblock.toggle': ({ host, on: enable }) => {
      if (!host) adblock = { ...adblock, enabled: enable };
      else if (enable) adblock = { ...adblock, allowlist: adblock.allowlist.filter((h) => h !== host) };
      else if (!adblock.allowlist.includes(host)) adblock = { ...adblock, allowlist: [...adblock.allowlist, host].sort() };
      return structuredClone(adblock);
    },
    'adblock.update_lists': async ({ lists, force }) => {
      await sleep(900);
      if (lists) adblock = { ...adblock, lists: [...lists] };
      return {
        compiled: true,
        compile_ms: 412,
        engine: { loaded: true, engine_bytes: 3_912_448, lists: [...adblock.lists, 'user'] },
        lists: adblock.lists.map((id, i) => ({
          id,
          url: `https://easylist.to/easylist/${id}.txt`,
          bytes: 1_400_000 - i * 310_000,
          status: force || i === 0 ? ('updated' as const) : ('not_modified' as const),
          error: null,
        })),
      };
    },
    'adblock.stats': ({ reset }) => {
      blocked += Math.round(Math.random() * 30);
      const total = {
        requests: 9_880 + blocked,
        blocked,
        by_filter: blocked - 140,
        by_policy: 140,
        redirected: 12,
        received_bytes: 84_000_000,
        blocked_by_type: { script: Math.round(blocked * 0.44), image: Math.round(blocked * 0.31), xhr: Math.round(blocked * 0.17), other: Math.round(blocked * 0.08) },
        blocked_hosts: { 'doubleclick.net': 412, 'googletagmanager.com': 233, 'facebook.net': 118, 'adnxs.com': 96, 'scorecardresearch.com': 41 },
      };
      if (reset) blocked = 0;
      return { engine: { loaded: true, engine_bytes: 3_912_448, lists: [...adblock.lists, 'user'] }, settings: structuredClone(adblock), tabs: [], total };
    },
    'simplify.list': () => ({ adblock: structuredClone(adblock), sites: structuredClone(sites) }),
    'simplify.get': ({ host }) => {
      const e = effective(host);
      return {
        host,
        adblock: adblock.enabled && !adblock.allowlist.some((h) => host === h || host.endsWith(`.${h}`)),
        matched: e.matched,
        policy: e.policy,
        content_settings: { javascript: e.policy.js ? 'allow' : 'block', images: e.policy.images ? 'allow' : 'block' },
      };
    },
    'simplify.site': ({ host, clear, ...fields }) => {
      if (!host.trim()) throw new RpcError('INVALID_PARAMS', 'host is empty');
      const stored: SitePolicy = clear ? {} : { ...(sites[host] ?? {}) };
      for (const [k, v] of Object.entries(fields)) if (v !== undefined) (stored as Record<string, unknown>)[k] = v;
      for (const k of Object.keys(stored) as (keyof SitePolicy)[]) if (stored[k] === null) delete stored[k];
      if (Object.keys(stored).length) sites[host] = stored;
      else delete sites[host];
      const e = effective(host === '*' ? 'example.com' : host);
      return {
        pattern: host,
        stored: structuredClone(stored),
        effective: e.policy,
        content_settings: { javascript: e.policy.js ? 'allow' : 'block', images: e.policy.images ? 'allow' : 'block' },
      };
    },

    // ---- plugins ----
    'plugins.list': () => ({ plugins: structuredClone(plugins) }),
    'plugins.enable': ({ name }) => {
      const p = plugin(name);
      p.enabled = true;
      p.status = p.manifest ? 'running' : 'error';
      p.pid = p.manifest ? 20_000 + Math.round(Math.random() * 9000) : null;
      p.restarts = 0;
      log(name, 'system', p.manifest ? `started (pid ${p.pid})` : 'cannot start: plugin.json is missing or invalid');
      return structuredClone(p);
    },
    'plugins.disable': ({ name }) => {
      const p = plugin(name);
      p.enabled = false;
      p.status = 'disabled';
      p.pid = null;
      log(name, 'system', 'stopped (disabled)');
      return structuredClone(p);
    },
    'plugins.reload': ({ name }) => {
      for (const p of plugins.filter((x) => !name || x.name === name)) {
        if (!p.enabled) continue;
        p.restarts = 0;
        p.status = p.manifest ? 'running' : 'crashed';
        log(p.name, 'system', 'reloaded');
      }
      return { plugins: structuredClone(plugins) };
    },
    'plugins.install': ({ path }) => {
      throw new RpcError('NOT_FOUND', `no plugin.json in ${path} (the demo backend cannot install plugins)`);
    },
    'plugins.logs': ({ name, limit }) => {
      plugin(name);
      return { name, lines: (logs.get(name) ?? []).slice(-(limit ?? 100)) };
    },
    'plugins.palette': () => ({
      commands: plugins
        .filter((p) => p.enabled && p.status === 'running')
        .flatMap((p) => (p.manifest?.ui.palette ?? []).map((e) => ({ plugin: p.name, id: e.id, title: e.title }))),
    }),
    'plugins.palette_run': ({ plugin: name, id }) => {
      const p = plugin(name);
      const entry = p.manifest?.ui.palette.find((e) => e.id === id);
      if (!entry) throw new RpcError('NOT_FOUND', `plugin ${name} has no palette entry ${id}`);
      const delivered = p.status === 'running';
      log(name, 'stdout', `plugin.command ${id}`);
      if (delivered) {
        setTimeout(
          () =>
            emit('ui.notification', {
              id: `n${Date.now()}`,
              title: entry.title,
              body: id === 'copy' ? 'Copied “8 min read” to the clipboard.' : 'About 8 minutes (1840 words).',
              level: 'info',
              actions: [],
              sticky: false,
              topic: 'plugin.command',
            }),
          300,
        );
      }
      return { delivered };
    },

    // ---- userscripts ----
    'userscripts.list': () => ({
      userscripts: [
        ...[...userCode.keys()].sort().map(userInfo),
        ...plugins
          .filter((p) => p.status === 'running')
          .flatMap((p) =>
            (p.manifest?.userscripts ?? []).map((file) => ({
              name: `${p.name}/${file.replace(/\.ts$/, '')}`,
              source: `plugin:${p.name}`,
              enabled: true,
              path: `${p.dir}\\${file}`,
              header: { matches: ['https://*/*'], includes: [], excludes: [], exclude_matches: [], run_at: 'document-idle' as const },
              last_run: null,
            })),
          ),
      ],
    }),
    'userscripts.get': ({ name }) => {
      const code = userCode.get(name);
      if (code === undefined) throw new RpcError('NOT_FOUND', `userscript ${name} not found`);
      return { name, code };
    },
    'userscripts.save': ({ name, code }) => {
      if (!/^[A-Za-z0-9_-]{1,64}$/.test(name)) throw new RpcError('INVALID_PARAMS', `bad userscript name "${name}": use 1-64 chars of A-Z a-z 0-9 _ - (no .ts)`);
      if (!headerOf(code)) throw new RpcError('INVALID_PARAMS', `userscript ${name}: no // @match or // @include line`);
      userCode.set(name, code);
      return userInfo(name);
    },
    'userscripts.delete': ({ name }) => {
      disabledUser.delete(name);
      return { removed: userCode.delete(name) };
    },
    'userscripts.enable': ({ name, enabled }) => {
      if (!userCode.has(name)) throw new RpcError('NOT_FOUND', `userscript ${name} not found`);
      const on = enabled ?? true;
      if (on) disabledUser.delete(name);
      else disabledUser.add(name);
      return { name, enabled: on };
    },
  };
}

// =============================================================================================
// Settings → Connections & API, Profiles & identity, Proxies, Recording & scripts
// (api.*, identity.*, proxy.*, geo.lookup, record.settings, script.settings). Kept in its own
// block: other settings sections add their fakes above.
// =============================================================================================

export type E2SettingsHandlers = {
  [M in keyof E2SettingsMethods]: (params: Params<M>) => Result<M> | Promise<Result<M>>;
};

type FullIdentity = Result<'identity.generate'>['identity'];

const E2_PRESETS: Result<'identity.presets'>['presets'] = [
  { name: 'windows-1080p', title: 'Windows 11 · Chrome · 1920×1080', os: 'windows', kind: 'stealth', device: null, screen: { width: 1920, height: 1080, dpr: 1 } },
  { name: 'windows-1440p', title: 'Windows 11 · Chrome · 2560×1440', os: 'windows', kind: 'stealth', device: null, screen: { width: 2560, height: 1440, dpr: 1 } },
  { name: 'macos-retina', title: 'macOS · Chrome · MacBook Pro 14"', os: 'macos', kind: 'stealth', device: null, screen: { width: 1512, height: 982, dpr: 2 } },
  { name: 'android-pixel-8', title: 'Android 14 · Chrome · Pixel 8', os: 'android', kind: 'stealth', device: 'Pixel 8', screen: { width: 412, height: 915, dpr: 2.625 } },
  { name: 'iphone-15', title: 'iPhone 15 · Safari (layout testing)', os: 'ios', kind: 'device_emulation', device: 'iPhone 15', screen: { width: 393, height: 852, dpr: 3 } },
  { name: 'ipad', title: 'iPad · Safari (layout testing)', os: 'ios', kind: 'device_emulation', device: 'iPad', screen: { width: 820, height: 1180, dpr: 2 } },
];

/** Where the fake exit IPs are, by a word in the proxy host. */
const E2_GEO: Record<string, Result<'geo.lookup'>['geo']> = {
  de: { ip: '185.12.64.20', country: 'Germany', country_code: 'DE', region: 'Berlin', city: 'Berlin', timezone: 'Europe/Berlin', latitude: 52.52, longitude: 13.405, isp: 'Mock Residential GmbH', locale: 'de-DE', accept_language: 'de-DE,de,en-US,en' },
  us: { ip: '104.28.3.9', country: 'United States', country_code: 'US', region: 'Virginia', city: 'Ashburn', timezone: 'America/New_York', latitude: 39.04, longitude: -77.49, isp: 'Mock Cloud Inc.', locale: 'en-US', accept_language: 'en-US,en' },
  ua: { ip: '91.214.10.4', country: 'Ukraine', country_code: 'UA', region: 'Kyiv City', city: 'Kyiv', timezone: 'Europe/Kyiv', latitude: 50.45, longitude: 30.52, isp: 'Mock Telecom', locale: 'uk-UA', accept_language: 'uk-UA,uk,en-US,en' },
};

function e2Identity(id: string, presetName: string, over: Partial<FullIdentity> = {}): FullIdentity {
  const preset = E2_PRESETS.find((p) => p.name === presetName) ?? E2_PRESETS[0];
  const mobile = preset.os === 'android' || preset.os === 'ios';
  const ua =
    preset.os === 'ios'
      ? 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1'
      : preset.os === 'android'
        ? 'Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Mobile Safari/537.36'
        : preset.os === 'macos'
          ? 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36'
          : 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36';
  return {
    id,
    preset: preset.name,
    kind: preset.kind,
    os: preset.os,
    user_agent: ua,
    ua_metadata: null,
    platform: preset.os === 'windows' ? 'Win32' : preset.os === 'macos' ? 'MacIntel' : preset.os === 'android' ? 'Linux armv81' : 'iPhone',
    hardware_concurrency: mobile ? 8 : 12,
    device_memory: 8,
    screen: preset.screen,
    viewport: mobile ? { width: preset.screen.width, height: preset.screen.height } : null,
    timezone: 'Europe/Kyiv',
    locale: 'uk-UA',
    accept_language: 'uk-UA,uk,en-US,en',
    geolocation: null,
    touch: mobile,
    max_touch_points: mobile ? 5 : 0,
    mobile,
    webgl_vendor: 'Google Inc. (NVIDIA)',
    webgl_renderer: 'ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)',
    ...over,
  };
}

const e2Mask = (url: string) => url.replace(/^(\w+:\/\/[^:@/]+):[^@/]*@/, '$1:***@');

/** A glob like the core's: `*.example.com` also matches `example.com`. */
function e2Matches(pattern: string, url: string): boolean {
  let host = '';
  try {
    host = new URL(url).hostname;
  } catch {
    return false;
  }
  if (pattern.startsWith('re:')) {
    try {
      return new RegExp(pattern.slice(3), 'i').test(url);
    } catch {
      return false;
    }
  }
  const glob = (p: string) => new RegExp(`^${p.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*')}$`, 'i');
  if (pattern.includes('://')) return glob(pattern).test(url);
  if (pattern.startsWith('*.') && host === pattern.slice(2)) return true;
  return glob(pattern).test(host);
}

export function createE2SettingsMocks(call: Call): E2SettingsHandlers {
  // ---- connections ----
  let connections = { cdp_enabled: false, cdp_port: 9222 };
  const cdpLaunch: { port: number | null; refused: boolean; from_flag: boolean } = { port: null, refused: false, from_flag: false };
  const cdpInfo = (): Result<'api.info'>['cdp'] => {
    const stealth = [...assigned.keys()].sort();
    const wanted = connections.cdp_enabled ? connections.cdp_port : null;
    const effective = stealth.length ? null : wanted;
    return {
      active_port: cdpLaunch.port,
      refused: cdpLaunch.refused,
      enabled: connections.cdp_enabled,
      port: connections.cdp_port,
      stealth_profiles: stealth,
      restart_needed: !cdpLaunch.from_flag && effective !== cdpLaunch.port,
      from_flag: cdpLaunch.from_flag,
    };
  };
  const TOOLSET_COUNTS: [string, number, boolean][] = [
    ['core', 31, true], ['net', 15, false], ['record', 8, false], ['script', 8, false], ['run', 10, false], ['profile', 12, false],
    ['stealth', 26, false], ['data', 22, false], ['ai', 9, false], ['ui', 6, false], ['system', 7, true],
  ];

  // ---- identities ----
  const identities = new Map<string, FullIdentity>([
    ['de-desktop', e2Identity('de-desktop', 'windows-1080p', { timezone: 'Europe/Berlin', locale: 'de-DE', accept_language: 'de-DE,de,en-US,en' })],
    ['phone-test', e2Identity('phone-test', 'iphone-15')],
  ]);
  /** profile → identity (mock.ts seeds stealth-de with de-desktop). */
  const assigned = new Map<string, string>([['stealth-de', 'de-desktop']]);
  const summary = (i: FullIdentity): Result<'identity.list'>['identities'][number] => ({
    id: i.id, preset: i.preset, os: i.os, kind: i.kind, user_agent: i.user_agent, timezone: i.timezone, locale: i.locale, screen: i.screen,
  });
  const requireIdentity = (id: string) => {
    const i = identities.get(id);
    if (!i) throw new RpcError('NOT_FOUND', `no saved identity "${id}"`);
    return i;
  };

  // ---- proxies ----
  type Pool = Result<'proxy.pool_list'>['pools'][number] & { real: string[] };
  const checked = Math.floor(Date.now() / 1000) - 3600;
  const pools = new Map<string, Pool>([
    ['residential-de', {
      name: 'residential-de', strategy: 'round_robin',
      real: ['socks5://u1:pw1@de1.pool.example:1080', 'socks5://u2:pw2@de2.pool.example:1080', 'socks5://u3:pw3@de3.pool.example:1080'],
      proxies: [],
    }],
    ['datacenter-us', { name: 'datacenter-us', strategy: 'random', real: ['http://us1.dc.example:3128', 'http://us2.dc.example:3128'], proxies: [] }],
  ]);
  const health = new Map<string, Result<'proxy.pool_check'>['proxies'][number]>();
  health.set('socks5://u1:pw1@de1.pool.example:1080', { proxy: '', healthy: true, latency_ms: 184, checked_at: checked, error: null });
  health.set('socks5://u2:pw2@de2.pool.example:1080', { proxy: '', healthy: true, latency_ms: 231, checked_at: checked, error: null });
  health.set('socks5://u3:pw3@de3.pool.example:1080', { proxy: '', healthy: false, latency_ms: null, checked_at: checked, error: 'connect timed out after 8000 ms' });
  const proxyRow = (url: string) => ({ ...(health.get(url) ?? { healthy: null, latency_ms: null, checked_at: null, error: null }), proxy: e2Mask(url) });
  const poolOut = (p: Pool) => ({ name: p.name, strategy: p.strategy, proxies: p.real.map(proxyRow) });
  const forwarders: Result<'proxy.forward_list'>['forwarders'] = [
    { id: 'fwd-1', socks5: 'socks5://127.0.0.1:53121', http: 'http://127.0.0.1:53121', upstream: 'http://corp-proxy:3128' },
  ];
  const routes = new Map<string, { rules: { match: string; via: string }[]; default: string }>([
    ['work', { rules: [{ match: '*.corp.local', via: 'direct' }], default: 'http://corp-proxy:3128' }],
    ['stealth-de', { rules: [], default: 'socks5://u:***@de.pool:1080' }],
  ]);
  const geoFor = (proxy: string | null | undefined) => {
    const p = (proxy ?? '').toLowerCase();
    if (!p || p === 'direct') return E2_GEO.ua;
    if (p.includes('us')) return E2_GEO.us;
    return E2_GEO.de;
  };

  // ---- recording & scripts ----
  const recordDefaults = { video_fps: 10, video_format: 'webm' as const, video_quality: 80 };
  let record: Result<'record.settings'>['settings'] = { ...recordDefaults };
  const scriptDefaults = { script_timeout_secs: 300, run_timeout_secs: 300, run_parallel: 4, run_retries: 1 };
  let scripts: Result<'script.settings'>['settings'] = { ...scriptDefaults };
  const EXE_DIR = 'C:\\Users\\you\\BrowserMCP';
  const int = (n: number | null | undefined, min: number, max: number, name: string) => {
    if (n == null) return;
    if (!Number.isInteger(n) || n < min || n > max) throw new RpcError('INVALID_PARAMS', `${name} must be ${min}-${max}, got ${n}`);
  };

  return {
    'api.info': () => ({
      version: '0.1.0',
      listening: true,
      port: 47800,
      urls: {
        mcp: 'http://127.0.0.1:47800/mcp',
        rest: 'http://127.0.0.1:47800/v1',
        ws: 'ws://127.0.0.1:47800/v1/ws',
        openapi: 'http://127.0.0.1:47800/openapi.json',
        health: 'http://127.0.0.1:47800/v1/health',
      },
      endpoint_file: `${DATA}\\runtime\\endpoint.json`,
      endpoint_present: true,
      exe: `${EXE_DIR}\\browsermcp.exe`,
      data_dir: DATA,
      toolsets: TOOLSET_COUNTS.map(([name, commands, def]) => ({ name, commands, default: def })),
      cdp: cdpInfo(),
    }),
    'api.config': ({ cdp_enabled, cdp_port }) => {
      int(cdp_port, 1024, 65535, 'cdp_port');
      if (cdp_enabled && assigned.size) {
        throw new RpcError('INVALID_PARAMS', `the CDP port cannot be opened while these profiles use a stealth identity: ${[...assigned.keys()].sort().join(', ')}`);
      }
      connections = { cdp_enabled: cdp_enabled ?? connections.cdp_enabled, cdp_port: cdp_port ?? connections.cdp_port };
      return { config: { ...connections }, cdp: cdpInfo() };
    },

    'record.settings': async (args) => {
      await sleep(80); // ffmpeg -version
      if (args.reset) record = { ...recordDefaults };
      int(args.video_fps, 1, 60, 'video_fps');
      int(args.video_quality, 1, 100, 'video_quality');
      record = {
        video_fps: args.video_fps ?? record.video_fps,
        video_format: args.video_format ?? record.video_format,
        video_quality: args.video_quality ?? record.video_quality,
      };
      return {
        settings: { ...record },
        defaults: { ...recordDefaults },
        ffmpeg: {
          found: true,
          path: 'C:\\Users\\you\\AppData\\Local\\Microsoft\\WinGet\\Links\\ffmpeg.exe',
          source: 'path',
          version: 'ffmpeg version 7.1-full_build-www.gyan.dev',
          env: null,
          exe_dir: EXE_DIR,
          install: 'winget install Gyan.FFmpeg',
        },
        recordings_dir: `${DATA}\\recordings`,
        videos_dir: `${DATA}\\videos`,
      };
    },
    'script.settings': (args) => {
      if (args.reset) scripts = { ...scriptDefaults };
      int(args.script_timeout_secs, 1, 86400, 'script_timeout_secs');
      int(args.run_timeout_secs, 1, 86400, 'run_timeout_secs');
      int(args.run_parallel, 1, 32, 'run_parallel');
      int(args.run_retries, 0, 10, 'run_retries');
      scripts = {
        script_timeout_secs: args.script_timeout_secs ?? scripts.script_timeout_secs,
        run_timeout_secs: args.run_timeout_secs ?? scripts.run_timeout_secs,
        run_parallel: args.run_parallel ?? scripts.run_parallel,
        run_retries: args.run_retries ?? scripts.run_retries,
      };
      return {
        settings: { ...scripts },
        defaults: { ...scriptDefaults },
        host: { found: true, path: `${EXE_DIR}\\bmcp-script-host.exe`, error: null, hint: null },
        scripts_dir: `${DATA}\\scripts`,
        runs_dir: `${DATA}\\runs`,
        max_parallel: 32,
        recommended_parallel: 6,
      };
    },

    'identity.presets': () => ({ presets: structuredClone(E2_PRESETS) }),
    'identity.list': () => ({
      identities: [...identities.values()].map(summary),
      profiles: Object.fromEntries(assigned),
    }),
    'identity.generate': ({ preset, os, id, save, timezone, locale }) => {
      const name = preset ?? E2_PRESETS.find((p) => p.os === (os ?? 'windows'))?.name ?? 'windows-1080p';
      if (!E2_PRESETS.some((p) => p.name === name)) throw new RpcError('NOT_FOUND', `no preset "${name}"`);
      const ident = e2Identity(id ?? `${name}-${Math.random().toString(16).slice(2, 10)}`, name, {
        ...(timezone ? { timezone } : {}),
        ...(locale ? { locale } : {}),
      });
      if (save) identities.set(ident.id, ident);
      return { identity: ident, seed: 42, saved: !!save, problems: [] };
    },
    'identity.assign': async ({ profile, id }) => {
      if (id) requireIdentity(id);
      // Like the core: never a stealth identity next to an open debugging port.
      if (id && cdpLaunch.port !== null) {
        throw new RpcError('INVALID_PARAMS', `a stealth identity cannot be used while the CDP port ${cdpLaunch.port} is open`);
      }
      if (id) assigned.set(profile, id);
      else assigned.delete(profile);
      await call('profile.create', { id: profile, identity: id ?? '' });
      return { profile, id: id ?? null };
    },
    'identity.remove': async ({ id }) => {
      const removed = identities.delete(id);
      for (const [profile, ident] of [...assigned]) {
        if (ident === id) {
          assigned.delete(profile);
          await call('profile.create', { id: profile, identity: '' });
        }
      }
      return { removed };
    },
    'identity.fit_to_proxy': async ({ identity_id, proxy_id, save }) => {
      await sleep(250);
      const ident = requireIdentity(identity_id);
      const geo = geoFor(proxy_id);
      const next = {
        ...ident,
        timezone: geo.timezone,
        locale: geo.locale ?? ident.locale,
        accept_language: geo.accept_language ?? ident.accept_language,
        geolocation: { latitude: geo.latitude + 0.01, longitude: geo.longitude - 0.01, accuracy: 30 },
      };
      const changes: { field: string; from: unknown; to: unknown }[] = (['timezone', 'locale', 'accept_language'] as const)
        .filter((k) => ident[k] !== next[k])
        .map((k) => ({ field: k, from: ident[k], to: next[k] }));
      changes.push({ field: 'geolocation', from: ident.geolocation ?? null, to: next.geolocation });
      if (save ?? true) identities.set(identity_id, next);
      return { identity_id, via: proxy_id === 'direct' ? null : proxy_id, geo, changes, saved: save ?? true, identity: next };
    },

    'proxy.pool_list': () => ({ pools: [...pools.values()].map(poolOut) }),
    'proxy.pool_add': ({ pool, proxies, strategy }) => {
      if (!/^[a-z0-9_-]{1,64}$/.test(pool)) throw new RpcError('INVALID_PARAMS', `pool "${pool}" is invalid: use 1-64 chars of a-z, 0-9, '-' or '_'`);
      for (const url of proxies) {
        if (/^(https|socks4a?):\/\//i.test(url)) throw new RpcError('INVALID_PARAMS', `bad proxy "${e2Mask(url)}": use http:// or socks5://`);
        if (!/:\d+(:[^:]+:[^:]+)?$/.test(url.replace(/@.*?:/, '@x:'))) throw new RpcError('INVALID_PARAMS', `bad proxy "${e2Mask(url)}": no port`);
      }
      const p = pools.get(pool) ?? { name: pool, strategy: strategy ?? 'round_robin', real: [], proxies: [] };
      const before = p.real.length;
      for (const url of proxies) if (!p.real.includes(url)) p.real.push(url);
      if (strategy) p.strategy = strategy;
      pools.set(pool, p);
      return { pool, added: p.real.length - before, total: p.real.length, strategy: p.strategy };
    },
    'proxy.pool_remove': ({ pool, proxy }) => {
      const p = pools.get(pool);
      if (!p) return { removed: false, remaining: 0 };
      if (proxy == null) {
        pools.delete(pool);
        return { removed: true, remaining: 0 };
      }
      const i = p.real.findIndex((u) => u === proxy || e2Mask(u) === proxy);
      if (i >= 0) p.real.splice(i, 1);
      return { removed: i >= 0, remaining: p.real.length };
    },
    'proxy.pool_check': async ({ pool }) => {
      const p = pools.get(pool);
      if (!p) throw new RpcError('NOT_FOUND', `no pool "${pool}"`);
      await sleep(600);
      const now = Math.floor(Date.now() / 1000);
      p.real.forEach((url, i) => {
        const bad = url.includes('de3') || url.includes('bad');
        health.set(url, { proxy: '', healthy: !bad, latency_ms: bad ? null : 120 + ((i * 67) % 240), checked_at: now, error: bad ? 'connect timed out after 8000 ms' : null });
      });
      const rows = p.real.map(proxyRow);
      return { pool, healthy: rows.filter((r) => r.healthy).length, unhealthy: rows.filter((r) => r.healthy === false).length, proxies: rows };
    },
    'proxy.forward_list': () => ({ forwarders: structuredClone(forwarders) }),
    'proxy.rules_get': ({ profile }) => ({
      routes: [...routes.entries()]
        .filter(([id]) => !profile || id === profile)
        .map(([id, r]) => ({ profile: id, rules: r.rules.map((x) => ({ ...x, via: e2Mask(x.via) })), default: e2Mask(r.default) })),
    }),
    'proxy.rules_set': ({ profile, rules, default: def }) => {
      for (const [i, r] of (rules ?? []).entries()) {
        if (!r.match.trim()) throw new RpcError('INVALID_PARAMS', `rule ${i + 1}: empty match`);
        if (!/^(direct|fwd-\d+|pool:[a-z0-9_-]+|lease:.+|(http|socks5h?):\/\/.+)$/.test(r.via)) throw new RpcError('INVALID_PARAMS', `rule ${i + 1}: unknown route "${r.via}"`);
        if (r.via.startsWith('pool:') && !pools.has(r.via.slice(5))) throw new RpcError('NOT_FOUND', `no pool "${r.via.slice(5)}"`);
      }
      const set = { rules: rules ?? [], default: def ?? 'direct' };
      if (!set.rules.length && set.default === 'direct') routes.delete(profile);
      else routes.set(profile, set);
      return {
        profile,
        rules: set.rules.map((x) => ({ ...x, via: e2Mask(x.via) })),
        default: e2Mask(set.default),
        pref: set.rules.length ? { mode: 'pac_script' } : set.default === 'direct' ? { mode: 'direct' } : { mode: 'fixed_servers' },
        pac_script: set.rules.length ? 'function FindProxyForURL(url, host) { /* mock */ return "DIRECT"; }' : null,
        forwarders: set.default === 'direct' ? [] : ['fwd-1'],
      };
    },
    'proxy.rules_test': ({ profile, url }) => {
      if (!/^\w+:\/\//.test(url)) throw new RpcError('INVALID_PARAMS', `"${url}" is not a URL`);
      const set = routes.get(profile) ?? { rules: [], default: 'direct' };
      const index = set.rules.findIndex((r) => e2Matches(r.match, url));
      const via = index >= 0 ? set.rules[index].via : set.default;
      const u = new URL(url);
      return {
        profile,
        pac_url: u.protocol === 'https:' ? `${u.protocol}//${u.host}/` : url,
        host: u.hostname,
        rule: index >= 0 ? index + 1 : null,
        matched: index >= 0 ? set.rules[index].match : null,
        via: e2Mask(via),
        upstream: via === 'direct' ? null : via.startsWith('pool:') ? e2Mask(pools.get(via.slice(5))?.real[0] ?? '') || null : e2Mask(via),
      };
    },
    'stealth.check': async ({ tab }) => {
      await sleep(400);
      const checks = [
        { name: 'navigator.webdriver', status: 'pass', detail: 'false' },
        { name: 'UA vs client hints', status: 'pass', detail: 'Chrome 156 on Windows in both' },
        { name: 'timezone vs identity', status: 'pass', detail: 'Europe/Berlin' },
        { name: 'WebRTC local IPs', status: 'pass', detail: 'none exposed (proxied)' },
        { name: 'fonts vs OS', status: 'warn', detail: '2 macOS-only fonts found' },
      ];
      const count = (s: string) => checks.filter((c) => c.status === s).length;
      return {
        tab: tab ?? 1,
        identity_id: 'de-desktop',
        passed: count('pass'),
        warned: count('warn'),
        failed: count('fail'),
        checks,
        text: checks.map((c) => `${c.status.toUpperCase().padEnd(5)} ${c.name}  ${c.detail}`).join('\n'),
      };
    },
    'geo.lookup': async ({ proxy, pool }) => {
      await sleep(300);
      if (proxy && /^(https|socks4a?):\/\//i.test(proxy)) throw new RpcError('INVALID_PARAMS', 'use http:// or socks5://');
      if (proxy?.includes('bad')) throw new RpcError('TIMEOUT', 'the proxy did not answer in 10000 ms');
      const via = proxy ? e2Mask(proxy) : pool ? e2Mask(pools.get(pool)?.real[0] ?? '') : null;
      return { via, geo: geoFor(proxy ?? (pool ? pools.get(pool)?.real[0] : null)) };
    },
  };
}
