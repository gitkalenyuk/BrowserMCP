import { getContext, setContext } from 'svelte';
import type { BrowserApi } from '../api/client';
import type { Agent, ConnectionStatus, TabId, Workspace, WorkspaceId } from '../api/types';
import { buildKeyIndex, commandForEvent, DEFAULT_KEYMAP, type CommandId, type Keymap, type KeyLike } from '../keymap';
import { AiStore } from '../ai/store.svelte';
import { chromeBookmarks } from '../data/chrome-bookmarks.svelte';
import { openLibrary } from '../data/library';
import type { NativeHost } from '../native';
import { NetStore } from '../net/store.svelte';
import { TerminalStore } from '../term/store.svelte';
import { settings, type PanelId } from '../settings.svelte';
import { NEWTAB_URL, resolveOmniboxInput } from '../urls';
import {
  activeTab,
  cycleTab,
  emptyTabs,
  groupsWithAgents,
  reduceTabs,
  tabByNumber,
  type TabsAction,
  type TabsState,
} from './tabs';

export interface ChromeOptions {
  /** The browser window around this page (null in the dev mock / a normal browser). */
  host?: NativeHost | null;
  /** `palette`: this controller runs the Ctrl+K overlay page, not the chrome itself. */
  role?: 'chrome' | 'palette';
}

/**
 * Reactive chrome state + every user intent. Components read state and call methods;
 * all side effects go through the BrowserApi (and, inside the real browser, the
 * window-local NativeHost for palette / focus / layout).
 */
export class ChromeController {
  readonly api: BrowserApi;
  readonly host: NativeHost | null;
  readonly role: 'chrome' | 'palette';
  /** Side panel state that outlives the panels (events arrive while they are closed). */
  readonly ai: AiStore;
  readonly terms: TerminalStore;
  readonly net: NetStore;
  tabs = $state<TabsState>(emptyTabs);
  agents = $state<Agent[]>([]);
  workspaces = $state<Workspace[]>([]);
  workspaceId = $state<WorkspaceId | null>(null);
  status = $state<ConnectionStatus>('connecting');
  paletteOpen = $state(false);
  /** Incremented to ask the omnibox to focus itself. */
  omniboxFocusTick = $state(0);
  lastError = $state<string | null>(null);
  keymap = $state<Keymap>(DEFAULT_KEYMAP);
  #keyIndex = $derived(buildKeyIndex(this.keymap));
  #closed: string[] = [];
  #unsubs: Array<() => void> = [];

  active = $derived(activeTab(this.tabs));

  constructor(api: BrowserApi, opts: ChromeOptions = {}) {
    this.api = api;
    this.host = opts.host ?? null;
    this.role = opts.role ?? 'chrome';
    this.ai = new AiStore(api);
    this.terms = new TerminalStore(api);
    this.net = new NetStore(api);
  }

  async start(): Promise<void> {
    const dispatch = (a: TabsAction) => (this.tabs = reduceTabs(this.tabs, a));
    const api = this.api;
    this.#unsubs.push(
      api.onStatus((s) => {
        this.status = s;
        if (s === 'open') void this.#refresh();
      }),
      api.on('tab.created', (payload) => dispatch({ type: 'created', payload })),
      api.on('tab.updated', (payload) => dispatch({ type: 'updated', payload })),
      api.on('tab.closed', (payload) => dispatch({ type: 'closed', payload })),
      api.on('tab.activated', (payload) => dispatch({ type: 'activated', payload })),
      api.on('tabs.reset', (snapshot) => dispatch({ type: 'reset', snapshot })),
      api.on('agents.updated', ({ agents }) => this.#setAgents(agents)),
      api.on('workspace.activated', ({ id }) => (this.workspaceId = id)),
    );
    if (this.role === 'chrome') {
      this.ai.start();
      this.terms.start();
      this.net.start();
    }
    await this.#guard(api.connect());
  }

  stop(): void {
    this.#unsubs.forEach((u) => u());
    this.#unsubs = [];
    this.ai.stop();
    this.terms.stop();
    this.net.stop();
  }

  async #refresh(): Promise<void> {
    // allSettled: the core may not implement agents/workspaces yet; tabs must still show.
    const [snapshot, agents, ws] = await Promise.allSettled([
      this.api.call('tabs.list'),
      this.api.call('agents.list'),
      this.api.call('workspaces.list'),
    ]);
    if (snapshot.status === 'fulfilled') {
      this.tabs = reduceTabs(this.tabs, { type: 'reset', snapshot: snapshot.value });
    } else {
      this.lastError = snapshot.reason instanceof Error ? snapshot.reason.message : String(snapshot.reason);
    }
    if (agents.status === 'fulfilled') this.#setAgents(agents.value.agents);
    if (ws.status === 'fulfilled') {
      this.workspaces = ws.value.workspaces;
      this.workspaceId = ws.value.activeId;
    }
  }

  /** New agent list: also refresh the agents' tab groups (a tab may join a new group). */
  #setAgents(agents: Agent[]): void {
    this.agents = agents;
    this.tabs = { ...this.tabs, groups: groupsWithAgents(this.tabs.groups, agents) };
  }

  async #guard<T>(p: Promise<T>): Promise<T | undefined> {
    try {
      const v = await p;
      this.lastError = null;
      return v;
    } catch (err) {
      this.lastError = err instanceof Error ? err.message : String(err);
      console.error('[chrome]', err);
      return undefined;
    }
  }

  // ---- tabs ------------------------------------------------------------------

  newTab(url: string = NEWTAB_URL): void {
    void this.#guard(this.api.call('tabs.open', { url, background: false }));
    if (url === NEWTAB_URL) this.focusOmnibox();
  }

  closeTab(id: TabId | null = this.tabs.activeId): void {
    if (id === null) return;
    const tab = this.tabs.tabs.find((t) => t.id === id);
    if (tab) this.#closed.push(tab.url);
    if (this.#closed.length > 25) this.#closed.shift();
    void this.#guard(this.api.call('tabs.close', { tab: id }));
  }

  reopenClosed(): void {
    const url = this.#closed.pop();
    if (url) this.newTab(url);
  }

  activate(id: TabId | null): void {
    if (id === null || id === this.tabs.activeId) return;
    // Optimistic: switch immediately, the core confirms with tab.activated.
    this.tabs = reduceTabs(this.tabs, { type: 'activated', payload: { id } });
    void this.#guard(this.api.call('tabs.activate', { tab: id }));
  }

  /** UI-only: collapsed tab groups (the active tab stays visible). */
  collapsed = $state<Record<string, boolean>>({});

  toggleGroup(id: string): void {
    this.collapsed = { ...this.collapsed, [id]: !this.collapsed[id] };
  }

  /** "Take over": the agent's lock on this tab pauses; the human is in control. */
  takeOver(id: TabId): void {
    void this.#guard(this.api.call('tabs.unlock', { tab: id }));
  }

  /** "Give back": a taken-over tab goes back to its agent. */
  giveBack(id: TabId): void {
    void this.#guard(this.api.call('tabs.lock', { tab: id }));
  }

  /** "Done" on a handoff banner: the agent continues in this tab. */
  finishHandoff(id: TabId): void {
    void this.#guard(this.api.call('tabs.unlock', { tab: id }));
  }

  takeOverAgent(agentId: string): void {
    for (const t of this.tabs.tabs) if (t.lockedBy === agentId) this.takeOver(t.id);
  }

  agent(id: string | null): Agent | undefined {
    return id ? this.agents.find((a) => a.id === id) : undefined;
  }

  // ---- navigation --------------------------------------------------------------

  navigate(input: string): boolean {
    const url = resolveOmniboxInput(input);
    if (!url) return false;
    const tab = this.tabs.activeId;
    if (tab === null) this.newTab(url);
    else {
      void this.#guard(this.api.call('nav.goto', { tab, url }));
      // Like Chrome: after Enter the keyboard belongs to the page.
      this.host?.focusContent();
    }
    return true;
  }

  #onActive(method: 'nav.back' | 'nav.forward' | 'nav.reload' | 'nav.stop'): void {
    const tab = this.tabs.activeId;
    if (tab !== null) void this.#guard(this.api.call(method, { tab }));
  }

  back = () => this.#onActive('nav.back');
  forward = () => this.#onActive('nav.forward');
  reload = () => this.#onActive('nav.reload');
  stopLoading = () => this.#onActive('nav.stop');

  /** Link hints are typed in the page, so the keyboard goes there. */
  showHints(): void {
    const tab = this.tabs.activeId;
    if (tab === null) return;
    void this.#guard(this.api.call('hints.show', { tab }));
    this.host?.focusContent();
  }

  focusOmnibox(): void {
    // The palette page has no omnibox: ask the window to focus the chrome's.
    if (this.role === 'palette' && this.host) this.host.focusOmnibox();
    else this.omniboxFocusTick++;
  }

  // ---- workspaces ------------------------------------------------------------------

  switchWorkspace(id: WorkspaceId): void {
    if (id === this.workspaceId) return;
    void this.#guard(this.api.call('workspaces.activate', { id }));
  }

  // ---- commands & keys -------------------------------------------------------------

  run(id: CommandId): void {
    const panel = (p: PanelId) => settings.togglePanel(p);
    switch (id) {
      case 'tab.new': return this.newTab();
      case 'tab.close': return this.closeTab();
      case 'tab.reopen': return this.reopenClosed();
      case 'tab.next': return this.activate(cycleTab(this.tabs, 1));
      case 'tab.prev': return this.activate(cycleTab(this.tabs, -1));
      case 'tab.select1': case 'tab.select2': case 'tab.select3': case 'tab.select4':
      case 'tab.select5': case 'tab.select6': case 'tab.select7': case 'tab.select8':
        return this.activate(tabByNumber(this.tabs, Number(id.slice(-1))));
      case 'tab.selectLast': return this.activate(tabByNumber(this.tabs, 9));
      case 'nav.back': return this.back();
      case 'nav.forward': return this.forward();
      case 'nav.reload': return this.active?.loading ? this.stopLoading() : this.reload();
      case 'nav.stop': return this.stopLoading();
      case 'omnibox.focus': return this.focusOmnibox();
      case 'hints.show': return this.showHints();
      case 'palette.open':
        // In the browser the palette is a separate overlay view (palette.html).
        if (this.host) this.host.palette(this.role === 'palette' ? 'hide' : 'toggle');
        else this.paletteOpen = !this.paletteOpen;
        return;
      case 'ui.toggleVerticalTabs': return settings.update({ verticalTabs: !settings.value.verticalTabs });
      case 'ui.toggleTheme': return settings.toggleTheme();
      case 'panel.traffic': return panel('traffic');
      case 'panel.recording': return panel('recording');
      case 'panel.ai': return panel('ai');
      case 'panel.terminal': return panel('terminal');
      case 'panel.scripts': return panel('scripts');
      case 'panel.runs': return panel('runs');
      case 'panel.close': return settings.update({ panel: null });
      case 'library.bookmarks': return openLibrary(this, 'bookmarks');
      case 'library.history': return openLibrary(this, 'history');
      case 'library.downloads': return openLibrary(this, 'downloads');
      case 'library.passwords': return openLibrary(this, 'passwords');
      case 'library.import': return openLibrary(this, 'import');
      case 'bookmark.toggle': return void chromeBookmarks(this.api).toggle(this.active?.url, this.active?.title);
      case 'ui.toggleBookmarksBar': return settings.update({ bookmarksBar: !settings.value.bookmarksBar });
    }
  }

  /** Global keydown handler. Returns true when the event was consumed. */
  handleKey(e: KeyLike): boolean {
    const id = commandForEvent(this.#keyIndex, e);
    if (!id) return false;
    // While the palette is open only its own toggle is global; it handles the rest.
    if (this.paletteOpen && id !== 'palette.open') return false;
    this.run(id);
    return true;
  }
}

const KEY = Symbol('chrome');

export function provideChrome(c: ChromeController): void {
  setContext(KEY, c);
}

export function useChrome(): ChromeController {
  return getContext<ChromeController>(KEY);
}
