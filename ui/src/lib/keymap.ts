// Single source of truth for keyboard shortcuts.
// DEFAULT_KEYMAP is the built-in map; data/config/keymap.json will later override it
// through parseKeymapOverrides() (same shape: { "command.id": ["Ctrl+X", ...] }, [] unbinds).

export const COMMAND_IDS = [
  'tab.new',
  'tab.close',
  'tab.reopen',
  'tab.next',
  'tab.prev',
  'tab.select1',
  'tab.select2',
  'tab.select3',
  'tab.select4',
  'tab.select5',
  'tab.select6',
  'tab.select7',
  'tab.select8',
  'tab.selectLast',
  'nav.back',
  'nav.forward',
  'nav.reload',
  'nav.stop',
  'omnibox.focus',
  'find.open',
  'hints.show',
  'palette.open',
  'ui.toggleVerticalTabs',
  'ui.toggleTheme',
  'panel.traffic',
  'panel.recording',
  'panel.ai',
  'panel.terminal',
  'panel.scripts',
  'panel.runs',
  'panel.close',
  'library.bookmarks',
  'library.history',
  'library.downloads',
  'library.passwords',
  'library.import',
  'bookmark.toggle',
  'ui.toggleBookmarksBar',
  'settings.open',
] as const;

export type CommandId = (typeof COMMAND_IDS)[number];
export type Keymap = Record<CommandId, string[]>;

export const DEFAULT_KEYMAP: Keymap = {
  'tab.new': ['Ctrl+T'],
  'tab.close': ['Ctrl+W', 'Ctrl+F4'],
  'tab.reopen': ['Ctrl+Shift+T'],
  'tab.next': ['Ctrl+Tab', 'Ctrl+PageDown'],
  'tab.prev': ['Ctrl+Shift+Tab', 'Ctrl+PageUp'],
  'tab.select1': ['Ctrl+1'],
  'tab.select2': ['Ctrl+2'],
  'tab.select3': ['Ctrl+3'],
  'tab.select4': ['Ctrl+4'],
  'tab.select5': ['Ctrl+5'],
  'tab.select6': ['Ctrl+6'],
  'tab.select7': ['Ctrl+7'],
  'tab.select8': ['Ctrl+8'],
  'tab.selectLast': ['Ctrl+9'],
  'nav.back': ['Alt+Left'],
  'nav.forward': ['Alt+Right'],
  'nav.reload': ['Ctrl+R', 'F5'],
  'nav.stop': [],
  'omnibox.focus': ['Ctrl+L', 'Alt+D', 'F6'],
  'find.open': ['Ctrl+F'],
  // In pages `f` / `F` show hints (crates/hints, configurable with hints.config).
  'hints.show': [],
  'palette.open': ['Ctrl+K'],
  'ui.toggleVerticalTabs': ['Ctrl+Shift+E'],
  'ui.toggleTheme': [],
  'panel.traffic': ['Ctrl+Shift+1'],
  'panel.recording': ['Ctrl+Shift+2'],
  'panel.ai': ['Ctrl+Shift+3'],
  'panel.terminal': ['Ctrl+`'],
  'panel.scripts': ['Ctrl+Shift+5'],
  'panel.runs': ['Ctrl+Shift+6'],
  'panel.close': [],
  'library.bookmarks': ['Ctrl+Shift+O'],
  'library.history': ['Ctrl+H'],
  'library.downloads': ['Ctrl+J'],
  'library.passwords': [],
  'library.import': [],
  'bookmark.toggle': ['Ctrl+D'],
  'ui.toggleBookmarksBar': ['Ctrl+Shift+B'],
  'settings.open': ['Ctrl+,'],
};

export interface Chord {
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  meta: boolean;
  /** Canonical lower-case key name: "t", "1", "tab", "arrowleft", "f5", "`", "space". */
  key: string;
}

const KEY_ALIASES: Record<string, string> = {
  esc: 'escape',
  return: 'enter',
  del: 'delete',
  ins: 'insert',
  left: 'arrowleft',
  right: 'arrowright',
  up: 'arrowup',
  down: 'arrowdown',
  pgup: 'pageup',
  pgdn: 'pagedown',
  ' ': 'space',
  spacebar: 'space',
  plus: '+',
  backquote: '`',
  backtick: '`',
};

const NAMED_KEYS = new Set([
  'escape', 'enter', 'tab', 'space', 'backspace', 'delete', 'insert', 'home', 'end',
  'pageup', 'pagedown', 'arrowleft', 'arrowright', 'arrowup', 'arrowdown',
]);

function canonicalKey(raw: string): string | null {
  const k = raw.toLowerCase();
  const key = KEY_ALIASES[k] ?? k;
  if (key.length === 1) return key;
  if (NAMED_KEYS.has(key)) return key;
  if (/^f([1-9]|1[0-9]|2[0-4])$/.test(key)) return key;
  return null;
}

/**
 * "Ctrl+Shift+Tab" -> Chord. Modifiers: Ctrl/Control, Shift, Alt/Option, Meta/Cmd/Win,
 * Mod (= Ctrl, or Meta on macOS). Returns null for invalid input.
 */
export function parseChord(text: string, opts: { mac?: boolean } = {}): Chord | null {
  const s = text.trim();
  if (!s) return null;
  // Split on '+' but keep a trailing literal '+' ("Ctrl++").
  const parts = s.endsWith('++') ? [...s.slice(0, -2).split('+'), '+'] : s.split('+');
  const chord: Chord = { ctrl: false, shift: false, alt: false, meta: false, key: '' };
  for (let i = 0; i < parts.length; i++) {
    const p = parts[i].trim();
    const lower = p.toLowerCase();
    const last = i === parts.length - 1;
    if (!last) {
      if (lower === 'ctrl' || lower === 'control') chord.ctrl = true;
      else if (lower === 'shift') chord.shift = true;
      else if (lower === 'alt' || lower === 'option') chord.alt = true;
      else if (lower === 'meta' || lower === 'cmd' || lower === 'win' || lower === 'super') chord.meta = true;
      else if (lower === 'mod') {
        if (opts.mac) chord.meta = true;
        else chord.ctrl = true;
      } else return null;
      continue;
    }
    const key = canonicalKey(p);
    if (!key) return null;
    chord.key = key;
  }
  return chord.key ? chord : null;
}

/** Canonical lookup string, e.g. "ctrl+shift+tab". */
export function chordId(c: Chord): string {
  return [c.ctrl && 'ctrl', c.alt && 'alt', c.shift && 'shift', c.meta && 'meta', c.key]
    .filter(Boolean)
    .join('+');
}

const CODE_KEYS: Record<string, string> = {
  Backquote: '`',
  Minus: '-',
  Equal: '=',
  BracketLeft: '[',
  BracketRight: ']',
  Backslash: '\\',
  Semicolon: ';',
  Quote: "'",
  Comma: ',',
  Period: '.',
  Slash: '/',
  Space: 'space',
};

export type KeyLike = Pick<KeyboardEvent, 'key' | 'code' | 'ctrlKey' | 'shiftKey' | 'altKey' | 'metaKey'>;

/**
 * KeyboardEvent -> Chord. Letters, digits and punctuation come from `code` so
 * shortcuts work on any layout (Ctrl+T on a Ukrainian layout reports key "е").
 */
export function chordFromEvent(e: KeyLike): Chord | null {
  let key: string | null = null;
  const code = e.code ?? '';
  let m: RegExpMatchArray | null;
  if ((m = code.match(/^Key([A-Z])$/))) key = m[1].toLowerCase();
  else if ((m = code.match(/^(?:Digit|Numpad)(\d)$/))) key = m[1];
  else if (CODE_KEYS[code]) key = CODE_KEYS[code];
  else if (e.key && !['Control', 'Shift', 'Alt', 'Meta', 'AltGraph', 'CapsLock'].includes(e.key)) {
    key = canonicalKey(e.key);
  }
  if (!key) return null;
  return { ctrl: e.ctrlKey, shift: e.shiftKey, alt: e.altKey, meta: e.metaKey, key };
}

export type KeyIndex = Map<string, CommandId>;

/** chordId -> command. Later entries win on conflicts (overrides are merged before). */
export function buildKeyIndex(keymap: Keymap, opts: { mac?: boolean } = {}): KeyIndex {
  const index: KeyIndex = new Map();
  for (const id of COMMAND_IDS) {
    for (const text of keymap[id] ?? []) {
      const chord = parseChord(text, opts);
      if (chord) index.set(chordId(chord), id);
    }
  }
  return index;
}

export function commandForEvent(index: KeyIndex, e: KeyLike): CommandId | null {
  const chord = chordFromEvent(e);
  return chord ? (index.get(chordId(chord)) ?? null) : null;
}

export interface KeymapParseResult {
  keymap: Keymap;
  errors: string[];
}

/**
 * Merges user overrides (parsed keymap.json) onto a base map. Unknown commands and
 * unparseable chords are reported and skipped; the rest still applies.
 */
export function parseKeymapOverrides(json: unknown, base: Keymap = DEFAULT_KEYMAP): KeymapParseResult {
  const keymap: Keymap = { ...base };
  const errors: string[] = [];
  if (json === null || typeof json !== 'object' || Array.isArray(json)) {
    return { keymap, errors: ['keymap.json must be an object of { "command.id": ["Ctrl+X"] }'] };
  }
  const known = new Set<string>(COMMAND_IDS);
  for (const [id, value] of Object.entries(json as Record<string, unknown>)) {
    if (!known.has(id)) {
      errors.push(`Unknown command "${id}"`);
      continue;
    }
    const list = typeof value === 'string' ? [value] : value;
    if (!Array.isArray(list) || list.some((v) => typeof v !== 'string')) {
      errors.push(`"${id}" must be a string or an array of strings`);
      continue;
    }
    const valid: string[] = [];
    for (const chord of list as string[]) {
      if (parseChord(chord)) valid.push(chord);
      else errors.push(`"${id}": cannot parse shortcut "${chord}"`);
    }
    keymap[id as CommandId] = valid;
  }
  return { keymap, errors };
}

const KEY_LABELS: Record<string, string> = {
  arrowleft: '←',
  arrowright: '→',
  arrowup: '↑',
  arrowdown: '↓',
  escape: 'Esc',
  pageup: 'PgUp',
  pagedown: 'PgDn',
  space: 'Space',
  enter: 'Enter',
  tab: 'Tab',
  delete: 'Del',
  backspace: 'Backspace',
};

/** Keycap labels for display: "Ctrl+Shift+Tab" -> ["Ctrl", "Shift", "Tab"]. */
export function chordLabels(text: string): string[] {
  const c = parseChord(text);
  if (!c) return [text];
  const keys: string[] = [];
  if (c.ctrl) keys.push('Ctrl');
  if (c.alt) keys.push('Alt');
  if (c.shift) keys.push('Shift');
  if (c.meta) keys.push('Win');
  keys.push(KEY_LABELS[c.key] ?? c.key.toUpperCase());
  return keys;
}
