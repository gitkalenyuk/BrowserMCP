// Event notifications (`{"jsonrpc":"2.0","method":"<topic>","params":{…}}` on /v1/ws).
// Hand-written: the payloads are built in Rust with `json!` (crates/core/src/command/registry.rs,
// crates/api/src/ws.rs), so they are not part of the registry dump. Keep in sync with
// crates/api/README.md ("Event topics").

/** Who called a command (Rust: `bmcp_core::Caller`). */
export type Caller =
  | { kind: "ui" }
  | { kind: "mcp"; session: string; agent: string | null }
  | { kind: "rest" }
  | { kind: "ws"; conn: number }
  | { kind: "script"; id: string }
  | { kind: "agent"; id: string }
  | { kind: "job"; run: string; step: number }
  | { kind: "keymap" };

/** Every event payload carries the bus sequence number (grows by one per event). */
export interface EventBase {
  seq: number;
}

/** Payloads of the topics BrowserMCP emits today, by topic. */
export interface Events {
  /** A command passed validation and the lock check and is about to run. */
  "command.started": EventBase & {
    call_id: number;
    name: string;
    caller: Caller;
    args: Record<string, unknown>;
    tab: number | null;
  };
  /** A started command finished; `error` is the error code or null. */
  "command.finished": EventBase & {
    call_id: number;
    name: string;
    caller: Caller;
    duration_ms: number;
    ok: boolean;
    error: string | null;
  };
  /** A call was refused before running (unknown command, bad arguments, tab locked). */
  "command.rejected": EventBase & {
    call_id: number;
    name: string;
    caller: Caller;
    args: unknown;
    error: string;
    message: string;
  };
  /** This connection fell behind and missed `missed` events: re-read state (WS only, no seq). */
  "bus.lagged": { missed: number };
}

export type EventTopic = keyof Events;

/** Topics a new WebSocket connection is subscribed to without asking (`command.*` is not). */
export const DEFAULT_TOPICS = ["tab.*", "tabs.*", "agents.*", "workspace.*", "runs.*"] as const;

/** `events.subscribe` / `events.unsubscribe` arguments: exact topics, `prefix.*` or `*`. */
export interface EventsSubscribeArgs {
  topics: string[];
}

/** The patterns active on the connection after (un)subscribing. */
export interface EventsSubscribeOutput {
  topics: string[];
}

/**
 * Does `pattern` match `topic`? Same rules as the server: `*` = everything, `tab.*` = `tab.x`
 * and `tab.x.y` (not `tabs.reset`), anything else = exact.
 */
export function topicMatches(pattern: string, topic: string): boolean {
  if (pattern === "*") return true;
  if (pattern.endsWith(".*")) return topic.startsWith(pattern.slice(0, -1));
  return pattern === topic;
}
