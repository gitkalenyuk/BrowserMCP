// Wire types shared by every BrowserApi implementation.
// These mirror what the Rust core will expose over JSON-RPC (ws://127.0.0.1:47800/v1/ws).
// Later they will be replaced by the generated @browsermcp/protocol package.

export type TabId = number;
export type GroupId = string;
export type AgentId = string;
export type WorkspaceId = string;

/** Named group colors (Chrome-like palette). The chrome maps them to CSS tokens. */
export type GroupColor =
  | 'grey'
  | 'blue'
  | 'red'
  | 'yellow'
  | 'green'
  | 'pink'
  | 'purple'
  | 'cyan'
  | 'orange';

/**
 * Agent lock of a tab (crates/core/src/agents.rs):
 * `active` — the agent drives the tab; `paused` — the human took over;
 * `handoff` — the agent waits for the human ("Done" gives the tab back).
 */
export interface TabLock {
  owner: AgentId;
  state: 'active' | 'paused' | 'handoff';
  /** handoff: what the agent asks the human to do. */
  reason: string | null;
  sinceMs: number;
}

export interface TabInfo {
  id: TabId;
  title: string;
  url: string;
  loading: boolean;
  /** Favicon URL (http(s) or data:), null when the page has none (yet). */
  favicon: string | null;
  canGoBack: boolean;
  canGoForward: boolean;
  groupId: GroupId | null;
  /** Agent currently holding the tab lock; null when the human is in control. */
  lockedBy: AgentId | null;
  /** Lock details; null/absent when no agent owns the tab. */
  lock?: TabLock | null;
  /** Sent by the core in tabs.list / tab.created (the UI uses TabsSnapshot.activeId instead). */
  active?: boolean;
}

export interface TabGroup {
  id: GroupId;
  name: string;
  color: GroupColor;
  /** Set for agent-owned groups ("Claude", "Codex"...). */
  agentId: AgentId | null;
}

export interface Agent {
  id: AgentId;
  name: string;
  color: GroupColor;
  status: 'idle' | 'running' | 'waiting' | 'error';
  /** Short description of what it is doing right now. */
  task: string | null;
  /** Its tab group (`agent:<id>`). Sent by the core. */
  groupId?: GroupId;
  /** Tabs in its group. Sent by the core. */
  tabs?: TabId[];
}

export interface TabsSnapshot {
  tabs: TabInfo[];
  groups: TabGroup[];
  activeId: TabId | null;
}

export interface Workspace {
  id: WorkspaceId;
  name: string;
  profile: string;
  /** Human readable proxy label, e.g. "Direct" or "socks5 · DE pool". */
  proxy: string;
  color: GroupColor;
}

export interface Run {
  id: string;
  name: string;
  status: 'queued' | 'running' | 'paused' | 'done' | 'failed';
  done: number;
  total: number;
  failed: number;
}

export interface Patch {
  id: string;
  runId: string;
  script: string;
  summary: string;
}

export interface Script {
  id: string;
  name: string;
  description: string;
  lastRun: string | null;
}

export interface Bookmark {
  title: string;
  /** Null for folders. */
  url: string | null;
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface ApiError {
  code: string;
  message: string;
  retry?: 'now' | 'after_wait' | 'after_snapshot' | 'no';
  hint?: string;
}

type TabParam = { tab: TabId };

/** Every JSON-RPC method the chrome calls: name -> params / result. */
export interface RpcMethods {
  'tabs.list': { params: void; result: TabsSnapshot };
  /** `background: true` opens without switching to it (default: the new tab becomes active). */
  'tabs.open': { params: { url?: string; background?: boolean }; result: TabInfo };
  'tabs.close': { params: TabParam; result: void };
  'tabs.activate': { params: TabParam; result: void };
  /**
   * Human: "Take over" an agent's tab (its lock pauses) or "Done" after a handoff (the agent
   * continues). Agent: release its own tab.
   */
  'tabs.unlock': { params: TabParam; result: { tab: TabId; lock: TabLock | null } };
  /** Human: give a taken-over tab back to its agent (or to `agent`). */
  'tabs.lock': { params: TabParam & { agent?: AgentId }; result: { tab: TabId; lock: TabLock | null } };
  'tabs.groups': { params: void; result: { groups: Array<TabGroup & { tabs: TabId[] }> } };
  'nav.goto': { params: TabParam & { url: string }; result: void };
  'nav.back': { params: TabParam; result: void };
  'nav.forward': { params: TabParam; result: void };
  'nav.reload': { params: TabParam; result: void };
  'nav.stop': { params: TabParam; result: void };
  'agents.list': { params: void; result: { agents: Agent[] } };
  /** Unlock the agent's tabs and drop its group. */
  'agents.forget': { params: { id: AgentId }; result: { released: TabId[] } };
  'workspaces.list': { params: void; result: { workspaces: Workspace[]; activeId: WorkspaceId } };
  'workspaces.activate': { params: { id: WorkspaceId }; result: void };
  'runs.list': { params: void; result: { runs: Run[]; patches: Patch[] } };
  'runs.patch.approve': { params: { id: string }; result: void };
  'runs.patch.reject': { params: { id: string }; result: void };
  'scripts.list': { params: void; result: Script[] };
  'scripts.run': { params: { id: string }; result: { runId: string } };
  // Shapes follow the Rust commands (crates/data): object outputs, folders have no url.
  'data.bookmarks': { params: void; result: { bookmarks: Bookmark[] } };
  'data.top_sites': { params: void; result: { sites: Bookmark[] } };
  'ai.prompt': { params: { text: string; provider: string }; result: { sessionId: string } };
  /** Reports the page area's bounds (CSS px, relative to the UI view) so the core can place the content view. */
  'ui.layout': { params: Rect & { dpr: number }; result: void };
}

/** Server -> client notifications. */
export interface ApiEvents {
  'tab.created': { tab: TabInfo; index: number };
  'tab.updated': { id: TabId } & Partial<Omit<TabInfo, 'id'>>;
  'tab.closed': { id: TabId };
  'tab.activated': { id: TabId };
  /** Whole tab set replaced (e.g. workspace switch). */
  'tabs.reset': TabsSnapshot;
  'agents.updated': { agents: Agent[] };
  /** An agent asks the human to do something in a tab (the banner shows it). */
  'ai.handoff': { tab: TabId; agent: AgentId; reason: string };
  'workspace.activated': { id: WorkspaceId };
  'runs.updated': { runs: Run[]; patches: Patch[] };
}

export type Method = keyof RpcMethods;
export type Params<M extends Method> = RpcMethods[M]['params'];
export type Result<M extends Method> = RpcMethods[M]['result'];
export type EventName = keyof ApiEvents;

/** `call('tabs.list')` for void params, `call('tabs.close', { tab })` otherwise. */
export type CallArgs<M extends Method> = Params<M> extends void ? [] : [params: Params<M>];

export type ConnectionStatus = 'connecting' | 'open' | 'closed';
