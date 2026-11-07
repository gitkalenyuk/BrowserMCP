import { getContext, setContext } from 'svelte';
import type { BrowserApi } from '../api/client';
import type { Agent, ConnectionStatus, PermissionPrompt, TabId, Workspace, WorkspaceId } from '../api/types';
import { buildKeyIndex, commandForEvent, type CommandId, type Keymap, type KeyLike } from '../keymap';
import { AiStore } from '../ai/store.svelte';
import { chromeBookmarks } from '../data/chrome-bookmarks.svelte';
import { openLibrary } from '../data/library';
import { openGuide, openSettings } from '../settings/open';
import { effectiveKeymap, onOverrides } from '../settings/keys';
import { windowKey, type NativeHost } from '../native';
import { NetStore } from '../net/store.svelte';
import { TerminalStore } from '../term/store.svelte';
import { settings, type PanelId } from '../settings.svelte';
import { NEWTAB_URL } from '../urls';
import { BrowserPrefs } from '../settings/e1-prefs.svelte';
import { resolveWithEngines } from '../settings/e1-search';
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
import {
  askAiPrompt,
  profileIdFor,
  tabForWorkspace,
  visibleTabs,
  workspaceOf,
  workspacesFromProfiles,
  type TabFilter,
} from './workspaces';

export interface ChromeOptions {
  /** The browser window around this page (null in the dev mock / a normal browser). */
  host?: NativeHost | null;
  /** This page's window key (default: `?window=` of the page URL). */
  window?: number | null;
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
  /** Every tab the core reported (all windows, all workspaces, off-screen ones too). */
  allTabs = $state<TabsState>(emptyTabs);
  agents = $state<Agent[]>([]);
  workspaces = $state<Workspace[]>([]);
  /** Workspace (= profile id) shown in this window. */
  workspaceId = $state<WorkspaceId | null>(null);
  /** Window key of this chrome page (null outside the browser: every tab is "ours"). */
  windowId: number | null = null;
  /** Permission requests of pages waiting for an answer (the permission bar). */
  prompts = $state<PermissionPrompt[]>([]);
  /** Open chrome dropdowns; while > 0 the page is hidden so they are visible. */
  #covers = 0;
  /** Last shown tab per workspace (switching back returns to it). */
  #lastActive: Record<string, TabId | undefined> = {};
  status = $state<ConnectionStatus>('connecting');
  paletteOpen = $state(false);
  /** Incremented to ask the omnibox to focus itself. */
  omniboxFocusTick = $state(0);
  /** Incremented to open the Ctrl+F find bar (FindBar.svelte). */
  findTick = $state(0);
  lastError = $state<string | null>(null);
  /** Defaults + the overrides saved on the settings page (Keyboard). */
  keymap = $state<Keymap>(effectiveKeymap());
  #keyIndex = $derived(buildKeyIndex(this.keymap));
  #closed: string[] = [];
  #unsubs: Array<() => void> = [];
  /** Settings → Search / Startup & tabs: address bar engines, new tab page, Home button. */
  prefs = new BrowserPrefs();

  /** The profile the browser started with (tabs without a `profile` belong to it). */
  launchProfile = $derived(this.workspaces.find((w) => w.launch)?.id ?? 'default');
  #filter: TabFilter = $derived({
    window: this.windowId,
    workspace: this.workspaceId,
    launch: this.launchProfile,
  });
  /** The tabs this window's strip shows: own window, current workspace, no off-screen tabs. */
  tabs: TabsState = $derived(visibleTabs(this.allTabs, this.#filter));
  active = $derived(activeTab(this.tabs));
  current = $derived(this.workspaces.find((w) => w.id === this.workspaceId) ?? null);

  constructor(api: BrowserApi, opts: ChromeOptions = {}) {
    this.api = api;
    this.host = opts.host ?? null;
    this.windowId = opts.window !== undefined ? opts.window : this.host ? windowKey() : null;
    this.role = opts.role ?? 'chrome';
    this.ai = new AiStore(api);
    this.terms = new TerminalStore(api);
    this.net = new NetStore(api);
  }

  async start(): Promise<void> {
    const dispatch = (a: TabsAction) => (this.allTabs = reduceTabs(this.allTabs, a));
    const api = this.api;
    this.#unsubs.push(
      this.prefs.start(api),
      api.onStatus((s) => {
        this.status = s;
        if (s === 'open') void this.#refresh();
      }),
      api.on('tab.created', (payload) => dispatch({ type: 'created', payload })),
      api.on('tab.updated', (payload) => dispatch({ type: 'updated', payload })),
      api.on('tab.closed', (payload) => dispatch({ type: 'closed', payload })),
      api.on('tab.activated', (payload) => {
        dispatch({ type: 'activated', payload });
        this.#followActive(payload.id);
      }),
      api.on('tabs.reset', (snapshot) => dispatch({ type: 'reset', snapshot })),
      api.on('agents.updated', ({ agents }) => this.#setAgents(agents)),
      api.on('permission.prompt', (p) => {
        this.prompts = [...this.prompts.filter((x) => x.id !== p.id), p];
      }),
      api.on('permission.prompt_closed', ({ id }) => {
        this.prompts = this.prompts.filter((x) => x.id !== id);
      }),
      api.on('ui.ask_ai', (p) => this.#askAi(p)),
      onOverrides(() => (this.keymap = effectiveKeymap())),
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
    // allSettled: the core may not implement agents/profiles; tabs must still show.
    const [snapshot, agents, prompts] = await Promise.allSettled([
      this.api.call('tabs.list'),
      this.api.call('agents.list'),
      this.api.call('permissions.pending'),
      this.refreshWorkspaces(),
    ]);
    if (snapshot.status === 'fulfilled') {
      this.allTabs = reduceTabs(this.allTabs, { type: 'reset', snapshot: snapshot.value });
      // The window shows the workspace of the tab on screen (e.g. after a restart).
      const shown = this.#ownActive();
      if (shown !== null) this.#followActive(shown);
    } else {
      this.lastError = snapshot.reason instanceof Error ? snapshot.reason.message : String(snapshot.reason);
    }
    if (agents.status === 'fulfilled') this.#setAgents(agents.value.agents);
    if (prompts.status === 'fulfilled') this.prompts = prompts.value.prompts;
  }

  /** Re-read the workspaces (profile.list). */
  async refreshWorkspaces(): Promise<void> {
    const { profiles } = await this.api.call('profile.list');
    this.workspaces = workspacesFromProfiles(profiles);
    if (this.workspaceId === null || !this.workspaces.some((w) => w.id === this.workspaceId)) {
      this.workspaceId = this.launchProfile;
    }
  }

  /** The core's active tab of this window (in any workspace). */
  #ownActive(): TabId | null {
    const id = this.allTabs.activeId;
    const tab = this.allTabs.tabs.find((t) => t.id === id);
    if (!tab || tab.offscreen) return null;
    if (this.windowId !== null && tab.window != null && tab.window !== this.windowId) return null;
    return tab.id;
  }

  /** A tab of this window became active: its workspace is the one shown. */
  #followActive(id: TabId): void {
    const tab = this.allTabs.tabs.find((t) => t.id === id);
    if (!tab || tab.offscreen) return;
    if (this.windowId !== null && tab.window != null && tab.window !== this.windowId) return;
    const ws = workspaceOf(tab, this.launchProfile);
    this.#lastActive[ws] = id;
    if (this.workspaces.length > 0 && ws !== this.workspaceId) this.workspaceId = ws;
    // A profile created elsewhere (API, agent): learn its name and color.
    if (this.workspaces.length > 0 && !this.workspaces.some((w) => w.id === ws)) {
      void this.refreshWorkspaces().catch(() => {});
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

  newTab(url: string = this.prefs.newTabUrl): void {
    // A new tab belongs to the workspace on screen (its profile: cookies, proxy, identity).
    const profile = this.workspaceId ?? undefined;
    const params = profile && this.workspaces.length > 0 ? { url, background: false, profile } : { url, background: false };
    void this.#guard(this.api.call('tabs.open', params));
    if (url === NEWTAB_URL || url === 'about:blank') this.focusOmnibox();
  }

  /** The Home button: the home page (Settings → Startup & tabs) in the shown tab. */
  home = (): void => {
    const url = this.prefs.homeUrl;
    const tab = this.tabs.activeId;
    if (tab === null) this.newTab(url);
    else void this.#guard(this.api.call('nav.goto', { tab, url }));
  };

  closeTab(id: TabId | null = this.tabs.activeId): void {
    if (id === null) return;
    const tab = this.tabs.tabs.find((t) => t.id === id);
    if (tab) this.#closed.push(tab.url);
    if (this.#closed.length > 25) this.#closed.shift();
    void this.#guard(this.api.call('tabs.close', { tab: id }));
  }

  /** Ctrl+Shift+T: the core's closed-tabs stack (URL, profile, place); the UI's own list
   *  only when the core does not have one (dev mock of an older core). */
  async reopenClosed(): Promise<void> {
    try {
      await this.api.call('tabs.restore_closed');
    } catch {
      const url = this.#closed.pop();
      if (url) this.newTab(url);
    }
  }

  activate(id: TabId | null): void {
    if (id === null || id === this.tabs.activeId) return;
    // Optimistic: switch immediately, the core confirms with tab.activated.
    this.allTabs = reduceTabs(this.allTabs, { type: 'activated', payload: { id } });
    void this.#guard(this.api.call('tabs.activate', { tab: id }));
  }

  // ---- split view, zoom ------------------------------------------------------------

  /** Split the active tab with its right neighbour (or left one), or end its split view. */
  toggleSplit(id: TabId | null = this.tabs.activeId): void {
    const tab = this.tabs.tabs.find((t) => t.id === id);
    if (!tab) return;
    if (tab.split) {
      void this.#guard(this.api.call('tabs.unsplit', { tab: tab.id }));
      return;
    }
    const i = this.tabs.tabs.indexOf(tab);
    const other = this.tabs.tabs[i + 1] ?? this.tabs.tabs[i - 1];
    if (!other) return;
    const [left, right] = this.tabs.tabs[i + 1] ? [tab.id, other.id] : [other.id, tab.id];
    void this.#guard(this.api.call('tabs.split', { left, right }));
  }

  zoom(delta: number): void {
    const tab = this.tabs.activeId;
    if (tab !== null) void this.#guard(this.api.call('page.zoom', { tab, delta }));
  }

  // ---- dropdowns over the page ----------------------------------------------------

  /** A chrome dropdown opened (true) or closed. Inside the browser the page is hidden while
   *  any is open: the native page view would otherwise cover it. */
  dropdown(open: boolean): void {
    const before = this.#covers;
    this.#covers = Math.max(0, this.#covers + (open ? 1 : -1));
    if ((before === 0) !== (this.#covers === 0)) this.host?.cover(this.#covers > 0);
  }

  // ---- permission bar ---------------------------------------------------------------

  answerPermission(id: number, allow: boolean, remember: boolean): void {
    this.prompts = this.prompts.filter((p) => p.id !== id);
    void this.#guard(this.api.call('permissions.answer', { id, allow, remember }));
  }

  // ---- AI --------------------------------------------------------------------------

  #askAi(p: { kind: string; text: string; url: string; title: string; tab: TabId }): void {
    if (this.role !== 'chrome') return;
    if (this.windowId !== null) {
      const tab = this.allTabs.tabs.find((t) => t.id === p.tab);
      if (tab?.window != null && tab.window !== this.windowId) return; // another window's page
    }
    this.ai.draft = askAiPrompt(p);
    if (settings.value.panel !== 'ai') settings.update({ panel: 'ai' });
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
    const url = resolveWithEngines(input, this.prefs.search);
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

  /** Show workspace `id` in this window: its last shown tab, or a new tab page in it. */
  switchWorkspace(id: WorkspaceId): void {
    if (id === this.workspaceId) return;
    const current = this.tabs.activeId;
    if (this.workspaceId !== null && current !== null) this.#lastActive[this.workspaceId] = current;
    this.workspaceId = id;
    const target = tabForWorkspace(this.allTabs.tabs, this.#filter, this.#lastActive);
    if (target !== null) this.activate(target);
    else this.newTab();
  }

  /** New workspace = new profile (own cookies and cache) named `name`; switches to it. */
  async createWorkspace(name: string, color?: string): Promise<void> {
    const label = name.trim();
    if (!label) return;
    const id = profileIdFor(label, this.workspaces.map((w) => w.id));
    await this.#guard(
      (async () => {
        await this.api.call('profile.create', { id });
        await this.api.call('profile.update', { id, label, ...(color ? { color } : {}) });
        await this.refreshWorkspaces();
      })(),
    );
    if (this.workspaces.some((w) => w.id === id)) this.switchWorkspace(id);
  }

  async renameWorkspace(id: WorkspaceId, name: string, color?: string): Promise<void> {
    const label = name.trim();
    await this.#guard(
      (async () => {
        await this.api.call('profile.update', { id, label, ...(color ? { color } : {}) });
        await this.refreshWorkspaces();
      })(),
    );
  }

  /** Close the workspace's tabs, then delete its profile (cookies, cache). Not the launch one. */
  async deleteWorkspace(id: WorkspaceId): Promise<void> {
    const ws = this.workspaces.find((w) => w.id === id);
    if (!ws || ws.launch) return;
    if (this.workspaceId === id) this.switchWorkspace(this.launchProfile);
    const tabs = this.allTabs.tabs.filter((t) => t.profile === id);
    await Promise.allSettled(tabs.map((t) => this.api.call('tabs.close', { tab: t.id })));
    // Tabs close asynchronously (beforeunload): retry the delete for a few seconds.
    for (let attempt = 0; attempt < 10; attempt++) {
      try {
        await this.api.call('profile.delete', { id });
        break;
      } catch (err) {
        if (attempt === 9) {
          this.lastError = err instanceof Error ? err.message : String(err);
          return;
        }
        await new Promise((r) => setTimeout(r, 300));
      }
    }
    await this.#guard(this.refreshWorkspaces());
  }

  // ---- commands & keys -------------------------------------------------------------

  run(id: CommandId): void {
    const panel = (p: PanelId) => settings.togglePanel(p);
    switch (id) {
      case 'tab.new': return this.newTab();
      case 'tab.close': return this.closeTab();
      case 'tab.reopen': return void this.reopenClosed();
      case 'tab.split': return this.toggleSplit();
      case 'split.focusLeft': return this.host?.focusPane('left');
      case 'split.focusRight': return this.host?.focusPane('right');
      case 'zoom.in': return this.zoom(1);
      case 'zoom.out': return this.zoom(-1);
      case 'zoom.reset': return this.zoom(0);
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
      case 'find.open': this.findTick++; return;
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
      case 'settings.open': return openSettings(this);
      case 'help.guide': return openGuide(this, 'start');
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
