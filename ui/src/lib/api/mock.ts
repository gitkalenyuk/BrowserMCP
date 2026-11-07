import { Emitter, RpcError, type BrowserApi } from './client';
import { createDataMocks } from './mock-data';
import { createLanguageMocks } from './mock-languages';
import { createVoiceMocks } from './mock-voice';
import { createPanelMocks } from './mock-panels';
import { createRecordRunsMocks } from './mock-record-runs';
import { createScriptMocks } from './mock-scripts';
import { createSettingsMocks } from './mock-settings';
import { createE1Mocks } from './mock-e1';
import { createModelMocks } from './mock-ai-models';
import type {
  Agent,
  ApiEvents,
  CallArgs,
  ConnectionStatus,
  EventName,
  Method,
  Params,
  ProfileEntry,
  Result,
  TabGroup,
  TabId,
  TabInfo,
  TabsSnapshot,
  WorkspaceId,
} from './types';
import { NEWTAB_URL, fallbackTitle, hostOf, isNewTab } from '../urls';

interface History {
  entries: string[];
  index: number;
}

/** The tabs of one profile (= workspace). The core has one strip per window; the mock keeps
 *  one per profile and reports them all (each tab carries its `profile`). */
interface WorkspaceState {
  tabs: TabInfo[];
  groups: TabGroup[];
  activeId: TabId | null;
}

const KNOWN_TITLES: Record<string, string> = {
  'github.com': 'GitHub',
  'developer.mozilla.org': 'MDN Web Docs',
  'svelte.dev': 'Svelte • Web development for the rest of us',
  'news.ycombinator.com': 'Hacker News',
  'docs.rs': 'Docs.rs',
  'crates.io': 'crates.io: Rust Package Registry',
  'example.com': 'Example Domain',
  'google.com': 'Google Search',
};

function titleFor(url: string): string {
  if (isNewTab(url)) return 'New Tab';
  const host = hostOf(url);
  if (url.includes('google.com/search')) {
    const q = new URL(url).searchParams.get('q');
    return q ? `${q} - Google Search` : 'Google';
  }
  const known = KNOWN_TITLES[host];
  if (known) {
    const path = new URL(url).pathname.split('/').filter(Boolean).slice(0, 2).join('/');
    return path ? `${path} · ${known}` : known;
  }
  return fallbackTitle(url);
}

type Handlers = { [M in Method]: (params: Params<M>) => Result<M> | Promise<Result<M>> };

/** The host a remembered zoom belongs to (like bmcp_core::zoom::host_of: www. is kept). */
function zoomHost(url: string): string {
  try {
    return /^https?:/i.test(url) ? new URL(url).hostname.toLowerCase() : '';
  } catch {
    return '';
  }
}

const AGENT_TOURS: Record<string, string[]> = {
  claude: [
    'https://www.bestbuy.com/site/laptops',
    'https://www.bhphotovideo.com/c/buy/laptops',
    'https://www.newegg.com/laptops-notebooks',
    'https://www.amazon.com/s?k=thinkpad+x1',
  ],
  codex: [
    'https://github.com/browsermcp/browsermcp/issues',
    'https://github.com/browsermcp/browsermcp/issues/42',
    'https://github.com/browsermcp/browsermcp/pulls',
  ],
};

/**
 * In-memory stand-in for the Rust core. Simulates loading, history, agents driving
 * their own tab groups, and run progress, so every chrome state can be exercised
 * without a backend. Not a spec — the core is the source of truth.
 */
export class MockApi implements BrowserApi {
  readonly kind = 'mock' as const;
  readonly status: ConnectionStatus = 'open';
  #events = new Emitter();
  #nextTab = 1;
  #history = new Map<TabId, History>();
  #profiles: ProfileEntry[];
  /** Profile of the tab shown last. */
  #activeWorkspace: WorkspaceId;
  #closedTabs: { url: string; profile: string; title?: string }[] = [];
  /** Remembered zoom per host (like data/config/zoom.json). */
  #zoomHosts = new Map<string, number>();
  #state = new Map<WorkspaceId, WorkspaceState>();
  #agents: Agent[];
  #timers: ReturnType<typeof setInterval>[] = [];
  #tourStep: Record<string, number> = { claude: 0, codex: 0 };
  #latency: number;

  constructor(opts: { latency?: number; simulate?: boolean } = {}) {
    this.#latency = opts.latency ?? 0;
    const profile = (id: string, label: string, extra: Partial<ProfileEntry> = {}): ProfileEntry => ({
      id,
      label,
      color: null,
      proxy: null,
      proxy_server: null,
      accept_language: null,
      identity: null,
      stored: true,
      open: true,
      incognito: false,
      tabs: [],
      launch: false,
      ...extra,
    });
    this.#profiles = [
      profile('default', 'Personal', { color: 'blue', launch: true }),
      profile('work', 'Work', { color: 'purple', proxy: 'http://corp-proxy:3128' }),
      profile('stealth-de', 'Scraping', { color: 'green', proxy: 'socks5://u:***@de.pool:1080', identity: 'de-desktop' }),
    ];
    this.#activeWorkspace = 'default';
    this.#agents = [
      {
        id: 'claude',
        name: 'Claude',
        color: 'orange',
        status: 'running',
        task: 'Comparing ThinkPad prices across 4 stores',
        groupId: 'g-claude',
      },
      { id: 'codex', name: 'Codex', color: 'green', status: 'running', task: 'Triaging new GitHub issues', groupId: 'g-codex' },
    ];
    this.#state.set('default', this.#seedPersonal());
    this.#state.set('work', this.#seed('work', [
      'https://github.com/browsermcp/browsermcp/pulls',
      'https://docs.rs/cef/latest/cef/',
      NEWTAB_URL,
    ]));
    this.#state.set('stealth-de', this.#seed('stealth-de', ['https://example.com/', NEWTAB_URL]));

    if (opts.simulate ?? true) this.#startSimulation();
  }

  get #ws(): WorkspaceState {
    return this.#state.get(this.#activeWorkspace)!;
  }

  connect(): Promise<void> {
    return Promise.resolve();
  }

  close(): void {
    this.#timers.forEach(clearInterval);
    this.#timers = [];
  }

  on<E extends EventName>(event: E, handler: (payload: ApiEvents[E]) => void): () => void {
    return this.#events.on(event, handler);
  }

  onStatus(handler: (status: ConnectionStatus) => void): () => void {
    handler(this.status);
    return () => {};
  }

  async call<M extends Method>(method: M, ...args: CallArgs<M>): Promise<Result<M>> {
    if (this.#latency) await new Promise((r) => setTimeout(r, this.#latency));
    const handler = this.#handlers[method] as (p: unknown) => Result<M> | Promise<Result<M>>;
    if (!handler) throw new RpcError('NOT_FOUND', `Unknown method ${method}`);
    // structuredClone: callers must never share objects with the "backend".
    return structuredClone(await handler(args[0]));
  }

  // ---- handlers -------------------------------------------------------------

  #handlers: Handlers = {
    'tabs.list': () => this.#snapshot(),
    'tabs.open': ({ url, background, profile }) => {
      const id = profile ?? 'default';
      if (!this.#profiles.some((p) => p.id === id)) throw new RpcError('INVALID_PARAMS', `No profile ${id}`);
      if (!this.#state.has(id)) this.#state.set(id, { tabs: [], groups: [], activeId: null });
      const ws = this.#state.get(id)!;
      const tab = this.#createTab(url ?? NEWTAB_URL, null, null, id);
      const at = ws.activeId === null ? ws.tabs.length : ws.tabs.findIndex((t) => t.id === ws.activeId) + 1;
      ws.tabs.splice(at, 0, tab);
      this.#emit('tab.created', { tab: { ...tab }, index: this.#snapshot().tabs.findIndex((t) => t.id === tab.id) });
      if (!background) this.#activate(tab.id);
      if (!isNewTab(tab.url)) this.#load(tab.id);
      return tab;
    },
    'tabs.close': ({ tab }) => {
      const key = this.#workspaceOf(tab);
      if (key === null) throw new RpcError('NOT_FOUND', `No tab ${tab}`);
      const ws = this.#state.get(key)!;
      const i = ws.tabs.findIndex((t) => t.id === tab);
      const [closed] = ws.tabs.splice(i, 1);
      this.#closedTabs.push({ url: closed.url, profile: key });
      this.#history.delete(tab);
      this.#emit('tab.closed', { id: tab });
      if (closed.split) this.#update(closed.split.other, { split: null });
      if (ws.tabs.length === 0) {
        ws.activeId = null;
        void this.#handlers['tabs.open']({ profile: key });
      } else if (ws.activeId === tab) {
        this.#activate(ws.tabs[Math.min(i, ws.tabs.length - 1)].id);
      }
    },
    'tabs.restore_closed': () => {
      const last = this.#closedTabs.pop();
      if (!last) return { tab: null };
      const tab = this.#handlers['tabs.open']({ url: last.url, profile: last.profile }) as TabInfo;
      return { tab: tab.id };
    },
    'tabs.closed': () => ({
      tabs: [...this.#closedTabs].reverse().map((c, index) => ({
        index,
        url: c.url,
        title: c.title ?? fallbackTitle(c.url),
        profile: c.profile === (this.#profiles.find((p) => p.launch)?.id ?? 'default') ? null : c.profile,
      })),
    }),
    'tabs.closed_reopen': ({ index }) => {
      const at = this.#closedTabs.length - 1 - index;
      if (index < 0 || at < 0) return { tab: null };
      const [entry] = this.#closedTabs.splice(at, 1);
      const tab = this.#handlers['tabs.open']({ url: entry.url, profile: entry.profile }) as TabInfo;
      return { tab: tab.id };
    },
    'tabs.closed_clear': () => {
      const cleared = this.#closedTabs.length;
      this.#closedTabs = [];
      return { cleared };
    },
    'zoom.list': () => ({
      sites: [...this.#zoomHosts].sort(([a], [b]) => a.localeCompare(b)).map(([host, percent]) => ({ host, percent })),
    }),
    'zoom.reset': (args) => {
      const host = args?.host?.trim().toLowerCase();
      const hosts = host ? (this.#zoomHosts.has(host) ? [host] : []) : [...this.#zoomHosts.keys()];
      for (const h of hosts) this.#zoomHosts.delete(h);
      for (const t of this.#snapshot().tabs) {
        if (hosts.includes(zoomHost(t.url))) this.#update(t.id, { zoom: null });
      }
      return { reset: hosts };
    },
    'tabs.split': ({ left, right, ratio }) => {
      this.#require(left);
      this.#require(right);
      const r = Math.min(0.85, Math.max(0.15, ratio ?? 0.5));
      this.#update(left, { split: { side: 'left', other: right, ratio: r } });
      this.#update(right, { split: { side: 'right', other: left, ratio: r } });
      this.#activate(left);
      return { left, right, ratio: r };
    },
    'tabs.unsplit': ({ tab }) => {
      const t = tab === undefined ? this.#snapshot().tabs.find((x) => x.split) : this.#require(tab);
      if (!t?.split) return { tabs: [] };
      const other = t.split.other;
      this.#update(t.id, { split: null });
      this.#update(other, { split: null });
      return { tabs: [t.id, other] };
    },
    'page.zoom': ({ tab, level, delta }) => {
      const id = tab ?? this.#ws.activeId ?? 0;
      const t = this.#require(id);
      const steps = [25, 33, 50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400, 500];
      const now = t.zoom ?? 100;
      let next = now;
      if (level !== undefined) next = level;
      else if (delta === 0) next = 100;
      else if (delta !== undefined && delta > 0) next = steps.find((s) => s > now) ?? 500;
      else if (delta !== undefined && delta < 0) next = [...steps].reverse().find((s) => s < now) ?? 25;
      this.#update(id, { zoom: next === 100 ? null : next });
      const host = zoomHost(t.url);
      if (host && next === 100) this.#zoomHosts.delete(host);
      else if (host) this.#zoomHosts.set(host, next);
      return { tab: id, level: next };
    },
    'profile.list': () => ({
      profiles: this.#profiles.map((p) => ({
        ...p,
        tabs: (this.#state.get(p.id)?.tabs ?? []).map((t) => t.id),
      })),
    }),
    'profile.create': ({ id, proxy, identity, accept_language, incognito_default, webrtc }) => {
      if (!/^[a-z0-9_-]{1,64}$/.test(id)) throw new RpcError('INVALID_PARAMS', `invalid profile id "${id}": use 1-64 chars of a-z, 0-9, '-' or '_'`);
      const created = !this.#profiles.some((p) => p.id === id);
      if (created) {
        this.#profiles.push({
          id, label: id, color: null, proxy: null, proxy_server: null, accept_language: null,
          identity: null, stored: true, open: false, incognito: false, tabs: [], launch: false,
        });
      }
      // Like the core: only the given fields change, "" removes, passwords come back masked.
      const p = this.#profiles.find((x) => x.id === id)!;
      if (proxy !== undefined) {
        const url = proxy.trim();
        p.proxy = url && url !== 'direct' ? url.replace(/^(\w+:\/\/[^:@/]+):[^@/]*@/, '$1:***@') : null;
        p.proxy_server = p.proxy ? 'socks5://127.0.0.1:53122' : null;
      }
      if (identity !== undefined) p.identity = identity || null;
      if (accept_language !== undefined) p.accept_language = accept_language.trim() || null;
      if (incognito_default !== undefined) p.incognito_default = incognito_default;
      if (webrtc !== undefined) p.webrtc = webrtc;
      return { id, created, incognito_default: p.incognito_default ?? false, webrtc: p.webrtc ?? 'auto' };
    },
    'profile.update': ({ id, label, color }) => {
      const p = this.#profiles.find((x) => x.id === id);
      if (!p) throw new RpcError('NOT_FOUND', `No profile ${id}`);
      if (label !== undefined) p.label = label.trim() || id;
      if (color !== undefined) p.color = (color || null) as ProfileEntry['color'];
      return { id, label: p.label, color: p.color };
    },
    'profile.delete': ({ id }) => {
      const p = this.#profiles.find((x) => x.id === id);
      if (!p) return { id, removed: false };
      if (p.launch) throw new RpcError('INVALID_PARAMS', 'the launch profile cannot be deleted');
      if ((this.#state.get(id)?.tabs.length ?? 0) > 0) throw new RpcError('INVALID_PARAMS', `profile "${id}" still has open tabs`);
      this.#profiles = this.#profiles.filter((x) => x.id !== id);
      this.#state.delete(id);
      return { id, removed: true };
    },
    'permissions.pending': () => ({ prompts: [] }),
    'permissions.answer': ({ id }) => ({ id, answered: false }),
    'tabs.activate': ({ tab }) => {
      this.#require(tab);
      this.#activate(tab);
    },
    // Same rules as the core (crates/core/src/agents.rs): Take over pauses the agent's lock,
    // Done ends a handoff, Give back (tabs.lock) re-activates it.
    'tabs.unlock': ({ tab }) => {
      const t = this.#require(tab);
      const owner = t.lock?.owner ?? t.lockedBy;
      if (!owner) return { tab, lock: null };
      const state = t.lock?.state ?? 'active';
      const next = state === 'handoff' ? 'active' : 'paused';
      const lock = { owner, state: next, reason: null, sinceMs: Date.now() } as const;
      this.#update(tab, { lockedBy: next === 'active' ? owner : null, lock });
      const stillOwns = [...this.#state.values()].some((s) => s.tabs.some((x) => x.lockedBy === owner));
      if (!stillOwns) {
        this.#agents = this.#agents.map((a) =>
          a.id === owner ? { ...a, status: 'idle', task: 'Paused — human took over' } : a,
        );
        this.#emit('agents.updated', { agents: structuredClone(this.#agents) });
      }
      return { tab, lock };
    },
    'tabs.lock': ({ tab, agent }) => {
      const t = this.#require(tab);
      const owner = agent ?? t.lock?.owner ?? t.lockedBy;
      if (!owner) throw new RpcError('INVALID_PARAMS', `tab ${tab} is not in an agent group; pass agent`);
      const lock = { owner, state: 'active', reason: null, sinceMs: Date.now() } as const;
      this.#update(tab, { lockedBy: owner, lock });
      this.#agents = this.#agents.map((a) => (a.id === owner ? { ...a, status: 'running', task: a.task } : a));
      this.#emit('agents.updated', { agents: structuredClone(this.#agents) });
      return { tab, lock };
    },
    'tabs.groups': () => ({
      groups: this.#ws.groups.map((g) => ({
        ...g,
        tabs: this.#ws.tabs.filter((t) => t.groupId === g.id).map((t) => t.id),
      })),
    }),
    'agents.forget': ({ id }) => {
      const released: TabId[] = [];
      for (const s of this.#state.values()) {
        for (const t of s.tabs) {
          if ((t.lock?.owner ?? t.lockedBy) === id) {
            released.push(t.id);
            this.#update(t.id, { lockedBy: null, lock: null, groupId: null });
          }
        }
      }
      this.#agents = this.#agents.filter((a) => a.id !== id);
      this.#emit('agents.updated', { agents: structuredClone(this.#agents) });
      return { released };
    },
    'nav.goto': ({ tab, url }) => {
      this.#require(tab);
      const h = this.#history.get(tab)!;
      h.entries = [...h.entries.slice(0, h.index + 1), url];
      h.index = h.entries.length - 1;
      this.#navigateTo(tab, url);
    },
    'nav.back': ({ tab }) => this.#step(tab, -1),
    'nav.forward': ({ tab }) => this.#step(tab, 1),
    'nav.reload': ({ tab }) => {
      this.#require(tab);
      this.#load(tab);
    },
    'nav.stop': ({ tab }) => {
      this.#require(tab);
      this.#update(tab, { loading: false });
    },
    'hints.show': () => ({ count: 0 }), // the mock has no page to label
    'agents.list': () => ({ agents: this.#agents }),
    ...createModelMocks(async (m, p) => (this.#handlers[m] as (x: unknown) => never)(p)),
    ...createDataMocks({
      empty: import.meta.env.VITE_BMCP_EMPTY_DATA === '1',
      emit: (event, payload) => this.#emit(event, payload),
    }),
    ...createScriptMocks((event, payload) => this.#emit(event, payload), {
      getCode: async (name) => (await this.#handlers['script.get']({ name })).code,
    }),
    ...createPanelMocks(
      (event, payload) => this.#emit(event, payload),
      () => this.#ws.activeId,
    ),
    ...createRecordRunsMocks((event, payload) => this.#emit(event, payload), () => this.#ws.activeId),
    ...createSettingsMocks((e, p) => this.#emit(e, p), (e, h) => this.on(e, h), async (m, p) => (this.#handlers[m] as (x: unknown) => never)(p)),
    ...createLanguageMocks(),
    ...createVoiceMocks(),
    ...createE1Mocks((e, p) => this.#emit(e, p), async (m, p) => (this.#handlers[m] as (x: unknown) => never)(p)),
    'ui.layout': (rect) => {
      console.info('[mock] ui.layout', rect);
    },
    // The mock has no page content: find in page never matches.
    'page.find_text': ({ tab }) => ({ tab: tab ?? 0, count: 0, index: 0, context: '' }),
    'page.find_clear': ({ tab }) => ({ tab: tab ?? 0 }),
    'ui.answer': ({ id, choice }) => {
      this.#emit('ui.notification_closed', { id, choice, label: choice, via: 'ui' });
      return { id, label: choice };
    },
  };

  // ---- helpers --------------------------------------------------------------

  #emit<E extends EventName>(event: E, payload: ApiEvents[E]): void {
    // Deliver asynchronously, like the real wire would.
    queueMicrotask(() => this.#events.emit(event, payload));
  }

  /** Every workspace's tabs (the chrome filters them by profile), the shown one active. */
  #snapshot(): TabsSnapshot {
    const all = [...this.#state.values()];
    return {
      tabs: all.flatMap((s) => s.tabs),
      groups: all.flatMap((s) => s.groups),
      activeId: this.#ws?.activeId ?? null,
    };
  }

  #workspaceOf(tab: TabId): string | null {
    for (const [key, s] of this.#state) if (s.tabs.some((t) => t.id === tab)) return key;
    return null;
  }

  #find(tab: TabId): TabInfo | undefined {
    for (const s of this.#state.values()) {
      const t = s.tabs.find((x) => x.id === tab);
      if (t) return t;
    }
    return undefined;
  }

  #require(tab: TabId): TabInfo {
    const t = this.#find(tab);
    if (!t) throw new RpcError('NOT_FOUND', `No tab ${tab}`);
    return t;
  }

  #createTab(url: string, groupId: string | null, lockedBy: string | null = null, profile = 'default'): TabInfo {
    const id = this.#nextTab++;
    this.#history.set(id, { entries: [url], index: 0 });
    return {
      id,
      url,
      title: titleFor(url),
      loading: false,
      favicon: null,
      canGoBack: false,
      canGoForward: false,
      groupId,
      lockedBy,
      profile,
    };
  }

  #seed(profile: string, urls: string[]): WorkspaceState {
    const tabs = urls.map((u) => this.#createTab(u, null, null, profile));
    return { tabs, groups: [], activeId: tabs[0]?.id ?? null };
  }

  #seedPersonal(): WorkspaceState {
    const groups: TabGroup[] = [
      { id: 'g-research', name: 'Research', color: 'blue', agentId: null },
      { id: 'g-claude', name: 'Claude', color: 'orange', agentId: 'claude' },
      { id: 'g-codex', name: 'Codex', color: 'green', agentId: 'codex' },
    ];
    const tabs = [
      this.#createTab('https://github.com/browsermcp/browsermcp', null),
      this.#createTab('https://developer.mozilla.org/en-US/docs/Web/API/WebSocket', 'g-research'),
      this.#createTab('https://svelte.dev/docs/svelte/$state', 'g-research'),
      this.#createTab(AGENT_TOURS.claude[0], 'g-claude', 'claude'),
      this.#createTab('https://www.lenovo.com/us/en/thinkpad-x1', 'g-claude', 'claude'),
      this.#createTab(AGENT_TOURS.codex[0], 'g-codex', 'codex'),
      this.#createTab('https://news.ycombinator.com/', null),
      this.#createTab(NEWTAB_URL, null),
    ];
    return { tabs, groups, activeId: tabs[tabs.length - 1].id };
  }

  #update(tab: TabId, patch: Partial<Omit<TabInfo, 'id'>>): void {
    const t = this.#find(tab);
    if (!t) return;
    Object.assign(t, patch);
    this.#emit('tab.updated', { id: tab, ...patch });
  }

  #activate(tab: TabId): void {
    const key = this.#workspaceOf(tab);
    if (key === null) return;
    const ws = this.#state.get(key)!;
    if (ws.activeId === tab && this.#activeWorkspace === key) return;
    ws.activeId = tab;
    this.#activeWorkspace = key;
    this.#emit('tab.activated', { id: tab });
  }

  #step(tab: TabId, delta: number): void {
    this.#require(tab);
    const h = this.#history.get(tab)!;
    const next = h.index + delta;
    if (next < 0 || next >= h.entries.length) return;
    h.index = next;
    this.#navigateTo(tab, h.entries[next]);
  }

  #navigateTo(tab: TabId, url: string): void {
    const h = this.#history.get(tab)!;
    this.#update(tab, {
      url,
      title: fallbackTitle(url),
      favicon: null,
      canGoBack: h.index > 0,
      canGoForward: h.index < h.entries.length - 1,
    });
    if (isNewTab(url)) {
      this.#update(tab, { title: 'New Tab', loading: false });
    } else {
      this.#load(tab);
    }
  }

  #load(tab: TabId): void {
    this.#update(tab, { loading: true });
    const ms = 450 + Math.random() * 900;
    setTimeout(() => {
      const t = this.#find(tab);
      if (!t || !t.loading) return; // closed or stopped
      this.#update(tab, { loading: false, title: titleFor(t.url) });
    }, ms);
  }

  #startSimulation(): void {
    // Agents keep browsing in their own locked tabs.
    this.#timers.push(
      setInterval(() => {
        for (const agent of this.#agents) {
          if (agent.status !== 'running') continue;
          const tour = AGENT_TOURS[agent.id];
          const tab = [...this.#state.values()]
            .flatMap((s) => s.tabs)
            .find((t) => t.lockedBy === agent.id);
          if (!tour || !tab) continue;
          this.#tourStep[agent.id] = (this.#tourStep[agent.id] + 1) % tour.length;
          void this.#handlers['nav.goto']({ tab: tab.id, url: tour[this.#tourStep[agent.id]] });
        }
      }, 6000),
    );
    // Runs (run.*) make progress in mock-record-runs.ts.
  }
}
