// The user guide's content model: chapters → sections → blocks, written as plain data in
// content/<locale>/*.ts and rendered by our own components (no markdown library, no {@html}).
//
// Inline text in blocks knows three marks, parsed by inline.ts:
//   `code`   a command, file or value       **bold**   emphasis       [[Ctrl+K]]   a key
//
// Builders below keep the content files short and typed: `api()` checks its arguments against
// the generated command types (sdk/protocol), `keys()` takes keymap ids, `settings()` takes a
// settings section id, so `pnpm -C ui check` catches a renamed command or section.

import type { CommandArgs, CommandName } from '../../../../sdk/protocol/src/generated/commands';
import type { CommandId } from '../../lib/keymap';
import type { SettingsSection } from '../../lib/settings/open';


export type Block =
  | { type: 'p'; text: string }
  /** Numbered steps. */
  | { type: 'steps'; items: string[] }
  /** Bullet list. */
  | { type: 'list'; items: string[] }
  | { type: 'tip'; text: string }
  | { type: 'warning'; text: string }
  | { type: 'code'; code: string; lang?: 'text' | 'bash' | 'powershell' | 'json' | 'ts' | 'toml' }
  /** Shortcuts: keymap commands (shown with the user's current keys) or literal keys. */
  | { type: 'keys'; items: KeyItem[] }
  /** A button that opens a settings section. */
  | { type: 'settings'; section: SettingsSection; text?: string }
  /** One command as an MCP tool call, PowerShell / curl request and SDK call. */
  | { type: 'api'; command: CommandName; args: Record<string, unknown>; text?: string }
  /** A link to another chapter (and section). */
  | { type: 'see'; chapter: string; section?: string; text?: string }
  /** Term → explanation pairs (glossary, file layouts). */
  | { type: 'terms'; items: { term: string; text: string }[] };

export type KeyItem = { command: CommandId; text: string } | { keys: string[]; text: string };

export interface Section {
  /** Stable id, the same in every language (deep links: #guide/<chapter>/<section>). */
  id: string;
  title: string;
  blocks: Block[];
}

export interface Chapter {
  /** Stable id, the same in every language. */
  id: string;
  title: string;
  /** One sentence for the table of contents and search results. */
  summary: string;
  sections: Section[];
}

export const p = (text: string): Block => ({ type: 'p', text });
export const steps = (...items: string[]): Block => ({ type: 'steps', items });
export const list = (...items: string[]): Block => ({ type: 'list', items });
export const tip = (text: string): Block => ({ type: 'tip', text });
export const warning = (text: string): Block => ({ type: 'warning', text });
export const code = (lang: NonNullable<Extract<Block, { type: 'code' }>['lang']>, text: string): Block => ({
  type: 'code',
  lang,
  code: text,
});
export const keys = (...items: KeyItem[]): Block => ({ type: 'keys', items });
/** A keymap command with its current shortcut. */
export const cmd = (command: CommandId, text: string): KeyItem => ({ command, text });
/** Keys that are not in the keymap (inside pages, editors, dialogs). */
export const key = (combo: string | string[], text: string): KeyItem => ({
  keys: Array.isArray(combo) ? combo : [combo],
  text,
});
export const settings = (section: SettingsSection, text?: string): Block => ({ type: 'settings', section, text });
export const see = (chapter: string, section?: string, text?: string): Block => ({ type: 'see', chapter, section, text });
export const terms = (...items: [term: string, text: string][]): Block => ({
  type: 'terms',
  items: items.map(([term, text]) => ({ term, text })),
});
/** A typed command example: `args` must match the command's generated argument type. */
export function api<N extends CommandName>(command: N, args: CommandArgs<N>, text?: string): Block {
  return { type: 'api', command, args: args as Record<string, unknown>, text };
}
export const section = (id: string, title: string, ...blocks: Block[]): Section => ({ id, title, blocks });
export const chapter = (id: string, title: string, summary: string, ...sections: Section[]): Chapter => ({
  id,
  title,
  summary,
  sections,
});

/** Chapter ids in reading order; every language has exactly these (guide.test.ts). */
export const CHAPTER_IDS = [
  'start',
  'tabs',
  'keyboard',
  'library',
  'newtab',
  'ai-panel',
  'voice',
  'connect',
  'perception',
  'traffic',
  'recording',
  'scripts',
  'runs',
  'stealth',
  'adblock',
  'performance',
  'notifications',
  'languages',
  'data',
  'troubleshooting',
  'glossary',
] as const;
