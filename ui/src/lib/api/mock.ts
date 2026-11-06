import { Emitter, RpcError, type BrowserApi } from './client';
import { createDataMocks } from './mock-data';
import { createPanelMocks } from './mock-panels';
import { createRecordRunsMocks } from './mock-record-runs';
import type {
  Agent,
  ApiEvents,
  CallArgs,
  ConnectionStatus,
  EventName,
  Method,
  Params,
  Patch,
  Result,
  Run,
  Script,
  TabGroup,
  TabId,
  TabInfo,
  TabsSnapshot,
  Workspace,
  WorkspaceId,
} from './types';
import { NEWTAB_URL, fallbackTitle, hostOf, isNewTab } from '../urls';

interface History {
  entries: string[];
  index: number;
}

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
  #workspaces: Workspace[];
  #activeWorkspace: WorkspaceId;
  #state = new Map<WorkspaceId, WorkspaceState>();
  #agents: Agent[];
  #runs: Run[];
  #patches: Patch[];
  #scripts: Script[];
  #timers: ReturnType<typeof setInterval>[] = [];
  #tourStep: Record<string, number> = { claude: 0, codex: 0 };
  #latency: number;

  constructor(opts: { latency?: number; simulate?: boolean } = {}) {
    this.#latency = opts.latency ?? 0;
    this.#workspaces = [
      { id: 'personal', name: 'Personal', profile: 'default', proxy: 'Direct', color: 'blue' },
      { id: 'work', name: 'Work', profile: 'work', proxy: 'http · corp-proxy:3128', color: 'purple' },
      { id: 'scrape', name: 'Scraping', profile: 'stealth-de', proxy: 'socks5 · DE pool (8)', color: 'green' },
    ];
    this.#activeWorkspace = 'personal';
    this.#agents = [
      { id: 'claude', name: 'Claude', color: 'orange', status: 'running', task: 'Comparing ThinkPad prices across 4 stores' },
      { id: 'codex', name: 'Codex', color: 'green', status: 'running', task: 'Triaging new GitHub issues' },
    ];
    this.#runs = [
      { id: 'run-1', name: 'Price watch · laptops.csv', status: 'running', done: 132, total: 500, failed: 3 },
      { id: 'run-2', name: 'Lead enrichment · 8 profiles', status: 'paused', done: 61, total: 240, failed: 0 },
      { id: 'run-3', name: 'Nightly screenshot diff', status: 'done', done: 48, total: 48, failed: 0 },
    ];
    this.#patches = [
      {
        id: 'patch-1',
        runId: 'run-2',
        script: 'enrich-leads.ts',
        summary: 'Selector "button.apply" not found — use getByRole("button", { name: "Apply filters" })',
      },
    ];
    this.#scripts = [
      { id: 's-login', name: 'Log in to staging', description: 'Fills credentials from vault, waits for dashboard', lastRun: '2h ago' },
      { id: 's-invoices', name: 'Download invoices', description: 'All PDFs from the billing page into runs/', lastRun: 'yesterday' },
      { id: 's-prices', name: 'Price watch', description: 'Checks 500 product URLs and exports CSV', lastRun: '10m ago' },
      { id: 's-clean', name: 'Clear site data', description: 'Cookies + storage for the current site', lastRun: null },
    ];

    this.#state.set('personal', this.#seedPersonal());
    this.#state.set('work', this.#seed([
      'https://github.com/browsermcp/browsermcp/pulls',
      'https://docs.rs/cef/latest/cef/',
      NEWTAB_URL,
    ]));
    this.#state.set('scrape', this.#seed(['https://example.com/', NEWTAB_URL]));

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
    'tabs.open': ({ url, background }) => {
      const tab = this.#createTab(url ?? NEWTAB_URL, null);
      const ws = this.#ws;
      const index = ws.activeId === null ? ws.tabs.length : this.#indexOf(ws.activeId) + 1;
      ws.tabs.splice(index, 0, tab);
      this.#emit('tab.created', { tab: { ...tab }, index });
      if (!background) this.#activate(tab.id);
      if (!isNewTab(tab.url)) this.#load(tab.id);
      return tab;
    },
    'tabs.close': ({ tab }) => {
      const ws = this.#ws;
      const i = this.#indexOf(tab);
      if (i < 0) throw new RpcError('NOT_FOUND', `No tab ${tab}`);
      ws.tabs.splice(i, 1);
      this.#history.delete(tab);
      this.#emit('tab.closed', { id: tab });
      if (ws.tabs.length === 0) {
        ws.activeId = null;
        void this.#handlers['tabs.open']({});
      } else if (ws.activeId === tab) {
        this.#activate(ws.tabs[Math.min(i, ws.tabs.length - 1)].id);
      }
    },
    'tabs.activate': ({ tab }) => {
      this.#require(tab);
      this.#activate(tab);
    },
    'tabs.unlock': ({ tab }) => {
      const t = this.#require(tab);
      if (!t.lockedBy) return;
      const agentId = t.lockedBy;
      this.#update(tab, { lockedBy: null });
      const stillOwns = [...this.#state.values()].some((s) => s.tabs.some((x) => x.lockedBy === agentId));
      if (!stillOwns) {
        this.#agents = this.#agents.map((a) =>
          a.id === agentId ? { ...a, status: 'idle', task: 'Paused — human took over' } : a,
        );
        this.#emit('agents.updated', { agents: structuredClone(this.#agents) });
      }
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
    'agents.list': () => this.#agents,
    'workspaces.list': () => ({ workspaces: this.#workspaces, activeId: this.#activeWorkspace }),
    'workspaces.activate': ({ id }) => {
      if (!this.#state.has(id)) throw new RpcError('NOT_FOUND', `No workspace ${id}`);
      this.#activeWorkspace = id;
      this.#emit('workspace.activated', { id });
      this.#emit('tabs.reset', this.#snapshot());
    },
    'runs.list': () => ({ runs: this.#runs, patches: this.#patches }),
    'runs.patch.approve': ({ id }) => {
      const patch = this.#patches.find((p) => p.id === id);
      if (!patch) throw new RpcError('NOT_FOUND', `No patch ${id}`);
      this.#patches = this.#patches.filter((p) => p.id !== id);
      this.#runs = this.#runs.map((r) => (r.id === patch.runId ? { ...r, status: 'running' } : r));
      this.#emitRuns();
    },
    'runs.patch.reject': ({ id }) => {
      const patch = this.#patches.find((p) => p.id === id);
      this.#patches = this.#patches.filter((p) => p.id !== id);
      if (patch) this.#runs = this.#runs.map((r) => (r.id === patch.runId ? { ...r, status: 'failed' } : r));
      this.#emitRuns();
    },
    'scripts.list': () => this.#scripts,
    'scripts.run': ({ id }) => {
      const script = this.#scripts.find((s) => s.id === id);
      if (!script) throw new RpcError('NOT_FOUND', `No script ${id}`);
      const runId = `run-${Date.now().toString(36)}`;
      this.#runs = [{ id: runId, name: script.name, status: 'running', done: 0, total: 20, failed: 0 }, ...this.#runs];
      this.#scripts = this.#scripts.map((s) => (s.id === id ? { ...s, lastRun: 'just now' } : s));
      this.#emitRuns();
      return { runId };
    },
    ...createDataMocks({ empty: import.meta.env.VITE_BMCP_EMPTY_DATA === '1' }),
    ...createPanelMocks(
      (event, payload) => this.#emit(event, payload),
      () => this.#ws.activeId,
    ),
    ...createRecordRunsMocks((event, payload) => this.#emit(event, payload), () => this.#ws.activeId),
    'ui.layout': (rect) => {
      console.info('[mock] ui.layout', rect);
    },
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

  #emitRuns(): void {
    this.#emit('runs.updated', structuredClone({ runs: this.#runs, patches: this.#patches }));
  }

  #snapshot(): TabsSnapshot {
    const ws = this.#ws;
    return { tabs: ws.tabs, groups: ws.groups, activeId: ws.activeId };
  }

  #indexOf(tab: TabId): number {
    return this.#ws.tabs.findIndex((t) => t.id === tab);
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

  #createTab(url: string, groupId: string | null, lockedBy: string | null = null): TabInfo {
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
    };
  }

  #seed(urls: string[]): WorkspaceState {
    const tabs = urls.map((u) => this.#createTab(u, null));
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
    const ws = this.#ws;
    if (ws.activeId === tab) return;
    ws.activeId = tab;
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
    // Runs make progress.
    this.#timers.push(
      setInterval(() => {
        let changed = false;
        this.#runs = this.#runs.map((r) => {
          if (r.status !== 'running') return r;
          changed = true;
          const done = Math.min(r.total, r.done + 1 + Math.floor(Math.random() * 4));
          const failed = r.failed + (Math.random() < 0.05 ? 1 : 0);
          return { ...r, done, failed, status: done >= r.total ? 'done' : 'running' };
        });
        if (changed) this.#emitRuns();
      }, 1200),
    );
  }
}
