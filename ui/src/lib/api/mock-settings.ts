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

export function createSettingsMocks(emit: Emit, on: On, call: Call): SettingsHandlers {
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
  const pendingOf = (proposal_id: string) => {
    const p = proposals.get(proposal_id);
    if (!p) throw new RpcError('NOT_FOUND', `no proposal ${proposal_id} (proposals are kept in memory)`);
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
      const p = pendingOf(proposal_id);
      const r = await call('run.heal_apply', { id: p.id, patched_code: p.patched_code });
      p.status = 'accepted';
      attempts.delete(p.id);
      emit('run.heal_applied', { id: p.id, proposal_id, script: r.script, requeued: r.requeued, auto: false });
      return { id: p.id, proposal_id, script: r.script, requeued: r.requeued };
    },
    'run.heal_reject': async ({ proposal_id, feedback }) => {
      const p = pendingOf(proposal_id);
      p.status = 'rejected';
      if ((attempts.get(p.id) ?? 0) >= p.max_attempts) {
        const error = `no usable patch after ${p.max_attempts} attempts`;
        emit('run.heal_failed', { id: p.id, row: p.row, attempts: p.max_attempts, error });
        throw new RpcError('INVALID_PARAMS', error);
      }
      const { patched_code: _code, ...next } = await makeProposal(await runOf(p.id), feedback ?? undefined);
      return next;
    },
    'run.heal_proposals': ({ id }) => ({
      proposals: [...proposals.values()].filter((p) => !id || p.id === id).sort((a, b) => b.created_ms - a.created_ms),
    }),

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
