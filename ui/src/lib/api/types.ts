// Wire types shared by every BrowserApi implementation.
export * from './types-data'; // data.* / bookmarks.* / history.* / … (merged into RpcMethods)
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
  /** Profile = workspace of the tab (absent: the launch profile / an old core). */
  profile?: string;
  incognito?: boolean;
  /** Off-screen automation tab: never shown in a tab strip. */
  offscreen?: boolean;
  /** Browser window showing the tab (absent for off-screen tabs). */
  window?: number;
  /** Set while the tab is one half of a split view (`tabs.split`). */
  split?: TabSplit | null;
  /** Page zoom in percent when it is not 100. */
  zoom?: number | null;
}

export interface TabSplit {
  side: 'left' | 'right';
  /** The tab in the other half. */
  other: TabId;
  /** Width share of the left half (0.15–0.85). */
  ratio: number;
}

/** Which addresses WebRTC may use in a profile (crates/profiles/src/prefs.rs). */
export type WebrtcPolicy = 'auto' | 'normal' | 'public_only' | 'proxy_only';

/** One entry of `profile.list` (crates/profiles/src/commands.rs). */
export interface ProfileEntry {
  id: string;
  /** Workspace name (profile.update label; default: the id). */
  label: string;
  color: GroupColor | null;
  /** Upstream proxy, password masked. */
  proxy: string | null;
  proxy_server: string | null;
  accept_language: string | null;
  identity: string | null;
  stored: boolean;
  open: boolean;
  incognito: boolean;
  /** New tabs of this profile open in memory unless tabs.open says otherwise. */
  incognito_default?: boolean;
  /** WebRTC setting (default auto) and the IP handling the browser got (null = not applied). */
  webrtc?: WebrtcPolicy;
  webrtc_applied?: string | null;
  tabs: TabId[];
  launch: boolean;
}

/** A page's permission request waiting for the permission bar. */
export interface PermissionPrompt {
  id: number;
  tab: TabId;
  origin: string;
  kinds: string[];
}

/** "Ask AI about this" from the page context menu. */
export interface AskAi {
  tab: TabId;
  url: string;
  title: string;
  kind: 'selection' | 'link' | 'image' | 'page';
  text: string;
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

/** A workspace = a profile (own cookies, proxy, identity), as the chrome shows it. */
export interface Workspace {
  id: WorkspaceId;
  name: string;
  profile: string;
  /** Human readable proxy label, e.g. "Direct" or "socks5 · DE pool". */
  proxy: string;
  color: GroupColor;
  /** Assigned stealth identity, if any. */
  identity?: string | null;
  /** The profile the browser started with (cannot be deleted). */
  launch?: boolean;
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
  'tabs.open': { params: { url?: string; background?: boolean; profile?: string }; result: TabInfo };
  'tabs.split': { params: { left: TabId; right: TabId; ratio?: number }; result: { left: TabId; right: TabId; ratio: number } };
  'tabs.unsplit': { params: { tab?: TabId }; result: { tabs: TabId[] } };
  'tabs.restore_closed': { params: void; result: { tab: TabId | null } };
  'page.zoom': { params: { tab?: TabId; level?: number; delta?: number }; result: { tab: TabId; level: number } };
  // ---- profiles = workspaces (crates/profiles) -----------------------------------------------
  'profile.list': { params: void; result: { profiles: ProfileEntry[] } };
  'profile.create': {
    /** proxy / identity / accept_language / incognito_default: only the fields to change ("" removes). */
    params: { id: string; proxy?: string; proxy_bypass?: string; identity?: string; accept_language?: string; incognito_default?: boolean; webrtc?: WebrtcPolicy };
    result: { id: string; created: boolean; incognito_default?: boolean; webrtc?: WebrtcPolicy; warning?: string | null };
  };
  'profile.update': { params: { id: string; label?: string; color?: string }; result: { id: string; label: string; color: string | null } };
  'profile.delete': { params: { id: string; keep_data?: boolean }; result: { id: string; removed: boolean } };
  'permissions.pending': { params: void; result: { prompts: PermissionPrompt[] } };
  'permissions.answer': {
    params: { id: number; allow: boolean; remember?: boolean };
    result: { id: number; answered: boolean };
  };
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
  /** Vim-style link hints in the page (crates/hints). */
  'hints.show': { params: TabParam & { mode?: 'click' | 'newtab' | 'focus' | 'copy' }; result: { count: number } };
  'agents.list': { params: void; result: { agents: Agent[] } };
  /** Unlock the agent's tabs and drop its group. */
  'agents.forget': { params: { id: AgentId }; result: { released: TabId[] } };
  // ---- built-in AI (crates/ai/src/commands.rs) --------------------------------------------
  'ai.providers': { params: void; result: { providers: AiProvider[] } };
  /** Omitted fields stay unchanged; "" clears one. Returns the provider. */
  'ai.provider_set': { params: AiProviderSet; result: AiProvider };
  /** Free check: API providers list their models, CLI providers run --version. */
  'ai.provider_test': { params: { id: string }; result: AiProviderTest };
  'ai.session_start': { params: AiSessionStart; result: AiSession };
  /** Models a provider offers for a new chat (CLI aliases, or the API's model list). */
  'ai.models': { params: { provider: string }; result: AiModels };
  'ai.session_send': { params: { id: string; text: string }; result: AiSession };
  'ai.session_stop': { params: { id: string }; result: AiSession };
  'ai.sessions_list': { params: void; result: { sessions: AiSession[] } };
  'ai.session_transcript': { params: { id: string }; result: { id: string; entries: AiTranscriptEntry[] } };
  // ---- terminals (crates/ai/src/term.rs) ---------------------------------------------------
  'term.open': { params: TermOpen; result: TermInfo };
  'term.write': { params: { id: string; data: string }; result: Empty };
  'term.resize': { params: { id: string; cols: number; rows: number }; result: Empty };
  'term.close': { params: { id: string }; result: Empty };
  'term.list': { params: void; result: { terminals: TermInfo[] } };
  // ---- traffic (crates/net/src/{commands,rule_commands}.rs) --------------------------------
  'net.capture': { params: { tab?: TabId; on: boolean; bodies?: boolean }; result: NetCaptureState };
  /** One text line per request (`#12 GET 200 xhr 3.4kB 120ms url`); see `parseNetList`. */
  'net.list': {
    params: { tab?: TabId; filter?: NetListFilter; cursor?: number; limit?: number };
    result: { tab: TabId; total: number; text: string; next_cursor: number; more: boolean };
  };
  'net.get': {
    params: { tab?: TabId; id: number; parts?: NetPart[]; max_bytes?: number };
    result: { tab: TabId; id: number; text: string; truncated: boolean };
  };
  'net.search': {
    params: { tab?: TabId; query: string; in?: 'url' | 'headers' | 'body'; limit?: number };
    result: { tab: TabId; total: number; text: string };
  };
  'net.ws_frames': {
    params: { tab?: TabId; id: number; cursor?: number; limit?: number; max_bytes?: number };
    result: { tab: TabId; id: number; total: number; text: string; next_cursor: number; more: boolean };
  };
  /** Without `path` the HAR comes back inline when it is at most 16 kB, else it is written to data/har/. */
  'net.har_export': {
    params: { tab?: TabId; path?: string; filter?: NetListFilter };
    result: { tab: TabId; entries: number; bytes: number; path?: string; har?: unknown };
  };
  'net.rule_add': { params: { tab?: TabId; match: NetMatch; action: NetAction }; result: NetRuleOutput };
  'net.breakpoint': { params: { tab?: TabId; match?: NetMatch; stage?: NetStage }; result: NetRuleOutput };
  /** `r1 tab 3 request GET glob "…" → block (2 hits)` per line; see `parseRuleLines`. */
  'net.rule_list': { params: { tab?: TabId }; result: NetLinesOutput };
  'net.rule_remove': { params: { id: string }; result: NetRuleOutput };
  /** `p3 b2 tab 1 request POST https://… (42s left)` per line; see `parsePausedLines`. */
  'net.paused_list': { params: { tab?: TabId }; result: NetLinesOutput };
  'net.resume': { params: NetResume; result: { pause_id: string; done: string; text: string } };
  // ---- find in page (crates/pagetools) ------------------------------------------------------
  'page.find_text': {
    params: { tab?: TabId; query: string; case?: boolean; next?: boolean; prev?: boolean };
    result: { tab: TabId; count: number; index: number; context: string };
  };
  'page.find_clear': { params: { tab?: TabId }; result: { tab: TabId } };
  /** Reports the page area's bounds (CSS px, relative to the UI view) so the core can place the content view. */
  'ui.layout': { params: Rect & { dpr: number }; result: void };
  /** A notification button was pressed (crates/notify). */
  'ui.answer': { params: { id: string; choice: string }; result: { id: string; label: string } };
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
  /** A page asks for a permission (geolocation, camera…): the permission bar shows it. */
  'permission.prompt': PermissionPrompt;
  'permission.prompt_closed': { id: number; allow: boolean | null };
  /** Page context menu "Ask AI about this". */
  'ui.ask_ai': AskAi;
  // ai.* / term.* / net.* (WsApi subscribes to them with events.subscribe on connect)
  'ai.message': AiMessageEvent;
  'ai.tool_call': { session: string; name: string; args: unknown };
  'ai.tool_result': { session: string; name: string; ok: boolean; summary: string };
  /** The agent finished answering one user message (`end_turn`, `max_steps`, `stopped`, `error: …`). */
  'ai.done': { session: string; reason: string };
  'ai.session': { session: string; status: AiSessionStatus };
  'term.output': { id: string; data: string };
  'term.exit': { id: string; code: number | null };
  'net.paused': NetPaused;
  'net.pause_timeout': { pause_id: string; tab: TabId; url: string; warning: string };
  'net.intercept_error': { tab: TabId; request_id: string; call: string; error: string; warning: string };
  'net.lagged': { tab: TabId; missed: number; warning: string };
  /** WS only: this connection fell behind the bus. */
  'bus.lagged': { missed: number };
  // ui.* (crates/notify): toasts and questions
  'ui.notification': UiNotification;
  /** Answered (in the UI or on Telegram), expired or resolved elsewhere: hide it. */
  'ui.notification_closed': { id: string; choice: string | null; label: string; via: string };
}

export type NotifyLevel = 'info' | 'success' | 'warning' | 'error';

/** `ui.notification`: a toast; with `actions` it is a question that stays until answered. */
export interface UiNotification {
  id: string;
  title: string;
  body: string;
  level: NotifyLevel;
  actions: { id: string; label: string }[];
  sticky: boolean;
  topic: string | null;
}

/** `Empty` output of the core (`{}`). */
export type Empty = Record<string, never> | void;

// ---- AI ---------------------------------------------------------------------------------

export interface AiProvider {
  /** `claude-code`, `codex`, `anthropic`, `openai`, `gemini`, `openrouter`, `mistral`, `deepseek`,
   *  `xai`, `groq`, `ollama`, `lmstudio`, `custom`. */
  id: string;
  name: string;
  /** `api` = BrowserMCP runs the agent loop; `cli` = Claude Code / Codex. */
  kind: 'api' | 'cli';
  protocol: 'anthropic' | 'openai' | 'gemini' | 'claude-code' | 'codex';
  /** Ready to start a session (key set / CLI installed). */
  configured: boolean;
  needs_key: boolean;
  /** An API key is stored (the key itself is never returned). */
  has_key: boolean;
  base_url: string;
  model: string | null;
  models: string[];
  /** CLI providers: the binary that will run. */
  path: string | null;
}

export interface AiProviderSet {
  id: string;
  api_key?: string;
  base_url?: string;
  model?: string;
  path?: string;
}

export interface AiProviderTest {
  id: string;
  ok: boolean;
  models: string[];
  version: string | null;
  error: string | null;
}

export type AiSessionStatus = 'running' | 'idle' | 'stopped' | 'failed';

export interface AiSession {
  /** Also the agent id (`anthropic-1`). */
  id: string;
  provider: string;
  model: string | null;
  /** Tab-group color, `#rrggbb`. */
  color: string;
  status: AiSessionStatus;
  tabs: number[];
  created_ms: number;
}

/** One model of `ai.models`. */
export interface AiModel {
  id: string;
  name: string;
  /** Short note ("most capable", "fast"), as the core sends it. */
  hint?: string;
}

export interface AiModels {
  models: AiModel[];
  /** The model the provider uses when none is given. */
  default?: string;
}

export interface AiSessionStart {
  provider: string;
  prompt: string;
  model?: string;
  toolsets?: string[];
  max_steps?: number;
}

/** `{session, role, text_delta}` while streaming, `{session, role, text}` for a whole message. */
export interface AiMessageEvent {
  session: string;
  role: 'user' | 'assistant' | 'system';
  text?: string;
  text_delta?: string;
  /** System notes the UI knows (`resume_lost`, `mcp_not_connected`): shown translated. */
  code?: string;
  params?: Record<string, string>;
}

export type AiTranscriptEntry = { ts_ms?: number } & (
  | { type: 'user' | 'assistant'; text: string }
  | { type: 'notice'; text: string; code?: string; params?: Record<string, string> }
  | { type: 'tool_call'; name: string; args: unknown }
  | { type: 'tool_result'; name: string; ok: boolean; summary: string }
  | { type: 'done'; reason: string }
  /** An answered (or cancelled) approval request. */
  | ({ type: 'permission' } & import('./types-ai-chat').AiPermission)
);

// ---- terminals --------------------------------------------------------------------------

export interface TermOpen {
  /** `pwsh`, `cmd`, `bash` or a program path; default pwsh (Windows) / $SHELL. */
  shell?: string;
  args?: string[];
  cwd?: string;
  cols?: number;
  rows?: number;
}

export interface TermInfo {
  id: string;
  shell: string;
  cwd: string;
  cols: number;
  rows: number;
  pid: number | null;
  exit_code: number | null;
}

// ---- traffic ----------------------------------------------------------------------------

export type NetStage = 'request' | 'response';
export type NetPart = 'headers' | 'req_body' | 'res_body';

export interface NetCaptureState {
  tab: TabId;
  capturing: boolean;
  bodies: boolean;
  entries: number;
}

export interface NetListFilter {
  url_contains?: string;
  method?: string;
  /** 404, "4xx", "failed" or "pending". */
  status?: number | string;
  /** Comma-separated resource types. */
  type?: string;
}

export interface NetMatch {
  url_glob?: string;
  url_regex?: string;
  method?: string;
  stage?: NetStage;
}

export type NetAction =
  | 'block'
  | { redirect: { url: string } }
  | { set_headers: Record<string, string> }
  | { remove_headers: string[] }
  | { rewrite_body: { find: string; replace: string } }
  | { mock: { status?: number; headers?: Record<string, string>; body?: string } };

export interface NetRuleOutput {
  id: string;
  text: string;
}

export interface NetLinesOutput {
  count: number;
  text: string;
}

export interface NetEdits {
  url?: string;
  method?: string;
  /** A null value removes the header. */
  headers?: Record<string, string | null>;
  body?: string;
  status?: number;
  response_body?: string;
}

export interface NetResume {
  pause_id: string;
  edits?: NetEdits;
  action?: 'continue' | 'drop';
}

/** `net.paused`: a breakpoint holds a request until `net.resume` (or 60 s). */
export interface NetPaused {
  pause_id: string;
  tab: TabId;
  /** Breakpoint id (`b2`). */
  rule: string;
  stage: NetStage;
  method: string;
  url: string;
  resource_type: string;
  /** CDP `Network.Headers`: name -> value. */
  headers: Record<string, string>;
  post_data?: string;
  /** Response stage only. */
  status?: number;
  /** Response stage only: CDP `Fetch.HeaderEntry[]`. */
  response_headers?: { name: string; value: string }[];
  error?: string;
}

export type Method = keyof RpcMethods;
export type Params<M extends Method> = RpcMethods[M]['params'];
export type Result<M extends Method> = RpcMethods[M]['result'];
export type EventName = keyof ApiEvents;

/** `call('tabs.list')` for void params, `call('tabs.close', { tab })` otherwise. */
export type CallArgs<M extends Method> = Params<M> extends void ? [] : [params: Params<M>];

export type ConnectionStatus = 'connecting' | 'open' | 'closed';

export type * from './types-record-runs';
// Scripts panel debugger (script.debug_* methods and events are merged into RpcMethods / ApiEvents there).
export type * from './types-scripts';
export type * from './types-settings'; // settings page + heal UI (notify, governor, hints, adblock, plugins, run.heal_*)
export type * from './types-ai-chat'; // AI chat: approvals, closing chats, agent access (merged into RpcMethods / ApiEvents)
export type * from './types-languages'; // browser.languages_* (Settings → General → Language)
export type * from './types-e2-settings'; // settings: connections (api.*), identities, proxies, recording/script defaults
