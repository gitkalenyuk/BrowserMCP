// Wire types of the AI chat's approvals, chat closing and agent access (crates/ai/src/commands.rs,
// crates/ai/src/cli/{access,permission}.rs). Params and results come from the generated
// protocol (sdk/protocol/src/generated/commands.ts, `cargo xtask gen-sdk`).
//
// types.ts re-exports this file; the `declare module` below adds the methods and events to
// RpcMethods / ApiEvents (declaration merging), and the new session fields to AiSession /
// AiSessionStart.

import type { Commands } from '../../../../sdk/protocol/src/generated/commands';

type Cmd<K extends keyof Commands> = { params: Commands[K]['args']; result: Commands[K]['output'] };

/** An approval request of a CLI agent (`ai.permissions_list`, `ai.permission` events). */
export type AiPermission = Commands['ai.permissions_list']['output']['permissions'][number];
export type AiPermissionStatus = AiPermission['status'];
export type AiPermissionDecision = Commands['ai.permission_answer']['args']['decision'];
/** What a CLI session runs with (`AiSession.access`). */
export type AiAccess = NonNullable<Commands['ai.sessions_list']['output']['sessions'][number]['access']>;
export type AiPermissionMode = AiAccess['permission_mode'];
export type AiCodexSandbox = AiAccess['sandbox'];
export type AiCodexApproval = AiAccess['approval_policy'];
/** A provider's defaults for new chats (`ai.agent_access`). */
export type AiAgentAccess = Commands['ai.agent_access']['output'];
export type AiAgentAccessSet = Commands['ai.agent_access_set']['args'];

export interface AiChatMethods {
  'ai.session_close': Cmd<'ai.session_close'>;
  'ai.permissions_list': Cmd<'ai.permissions_list'>;
  'ai.permission_answer': Cmd<'ai.permission_answer'>;
  'ai.agent_access': Cmd<'ai.agent_access'>;
  'ai.agent_access_set': Cmd<'ai.agent_access_set'>;
}

export interface AiChatEvents {
  /** A CLI agent asks before a tool call (`pending`), or how that ended. */
  'ai.permission': AiPermission;
  /** Removed with ai.session_close (in this or another window). */
  'ai.session_closed': { session: string };
}

declare module './types' {
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  interface RpcMethods extends AiChatMethods {}
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  interface ApiEvents extends AiChatEvents {}
  interface AiSession {
    /** CLI agents: permission mode, sandbox and folders in effect; null for API agents. */
    access?: AiAccess | null;
    /** Approval requests waiting for an answer. */
    pending_permissions?: number;
  }
  interface AiSessionStart {
    permission_mode?: AiPermissionMode;
    cwd?: string;
    add_dirs?: string[];
    sandbox?: AiCodexSandbox;
    approval_policy?: AiCodexApproval;
  }
}
