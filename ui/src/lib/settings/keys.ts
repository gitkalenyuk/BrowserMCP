// Keyboard section: the keymap with search, and the user's overrides. Until the core stores
// data/config/keymap.json the overrides live in localStorage (`bmcp.keymap.overrides`, same
// shape as keymap.json: {"command.id": ["Ctrl+X"]}, [] unbinds); the chrome merges them with
// parseKeymapOverrides and follows changes through the storage event.

import { COMMANDS } from '../commands';
import { t } from '../i18n/index.svelte';
import {
  COMMAND_IDS,
  DEFAULT_KEYMAP,
  chordFromEvent,
  chordId,
  parseChord,
  parseKeymapOverrides,
  type Chord,
  type CommandId,
  type KeyLike,
  type Keymap,
} from '../keymap';
import { load, save, watch } from '../storage';

export type KeymapOverrides = Partial<Record<CommandId, string[]>>;

const KEY = 'keymap.overrides';

export function loadOverrides(): KeymapOverrides {
  const raw = load<unknown>(KEY, {});
  return raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as KeymapOverrides) : {};
}

export function saveOverrides(o: KeymapOverrides): void {
  save(KEY, o);
}

export function onOverrides(cb: () => void): () => void {
  return watch(KEY, cb);
}

/** The defaults with the stored overrides applied (bad entries are skipped). */
export function effectiveKeymap(o: KeymapOverrides = loadOverrides()): Keymap {
  return parseKeymapOverrides(o).keymap;
}

/** Set the shortcuts of one command; back to the default ones drops the override. */
export function setBinding(o: KeymapOverrides, id: CommandId, chords: string[]): KeymapOverrides {
  const next = { ...o };
  if (sameChords(chords, DEFAULT_KEYMAP[id])) delete next[id];
  else next[id] = chords;
  return next;
}

export function resetBinding(o: KeymapOverrides, id: CommandId): KeymapOverrides {
  const next = { ...o };
  delete next[id];
  return next;
}

function sameChords(a: string[], b: string[]): boolean {
  const norm = (l: string[]) =>
    l
      .map((t) => parseChord(t))
      .filter((c): c is Chord => !!c)
      .map(chordId)
      .sort()
      .join(' ');
  return norm(a) === norm(b);
}

/** "Ctrl+Shift+K" for a chord (the text form keymap.json uses). */
export function chordText(c: Chord): string {
  const names: Record<string, string> = {
    arrowleft: 'Left',
    arrowright: 'Right',
    arrowup: 'Up',
    arrowdown: 'Down',
    escape: 'Esc',
    pageup: 'PageUp',
    pagedown: 'PageDown',
    space: 'Space',
    enter: 'Enter',
    tab: 'Tab',
    delete: 'Delete',
    backspace: 'Backspace',
    insert: 'Insert',
    home: 'Home',
    end: 'End',
  };
  const key = names[c.key] ?? (c.key.length === 1 ? c.key.toUpperCase() : c.key.toUpperCase());
  return [c.ctrl && 'Ctrl', c.alt && 'Alt', c.shift && 'Shift', c.meta && 'Meta', key].filter(Boolean).join('+');
}

/**
 * The shortcut a key press records, or null while only modifiers are down. Plain letters,
 * digits and punctuation need a modifier (they type text); F-keys and named keys do not.
 */
export function recordChord(e: KeyLike): string | null {
  const c = chordFromEvent(e);
  if (!c) return null;
  const plain = !c.ctrl && !c.alt && !c.meta;
  if (plain && (c.key.length === 1 || c.key === 'space' || c.key === 'enter' || c.key === 'backspace')) return null;
  return chordText(c);
}

/** Chords bound to more than one command: chordId → commands. */
export function conflicts(keymap: Keymap): Map<string, CommandId[]> {
  const by = new Map<string, CommandId[]>();
  for (const id of COMMAND_IDS) {
    for (const text of keymap[id] ?? []) {
      const c = parseChord(text);
      if (!c) continue;
      const k = chordId(c);
      by.set(k, [...(by.get(k) ?? []), id]);
    }
  }
  return new Map([...by].filter(([, ids]) => ids.length > 1));
}

export interface KeymapRow {
  id: CommandId;
  /** In the interface language. */
  title: string;
  /** The English title (lib/commands.ts), also searched so "history" works in any language. */
  english: string;
  keys: string[];
  custom: boolean;
}

const TITLES = new Map(COMMANDS.map((c) => [c.id, c.title]));

/**
 * A command's title in the interface language: `settings.keyboard.cmd.<id>` (the eight
 * "Select tab N" commands share `cmd.tab.selectN`), else the English title from lib/commands.ts
 * (a command added there before its translation still shows up).
 */
export function commandTitle(id: CommandId): string {
  const n = /^tab\.select(\d)$/.exec(id)?.[1];
  const key = n ? 'settings.keyboard.cmd.tab.selectN' : `settings.keyboard.cmd.${id}`;
  const text = t(key, n ? { n } : undefined);
  return text === key ? (TITLES.get(id) ?? id) : text;
}

/** Every command with its title and current keys. */
export function keymapRows(keymap: Keymap, o: KeymapOverrides): KeymapRow[] {
  return COMMAND_IDS.map((id) => ({
    id,
    title: commandTitle(id),
    english: TITLES.get(id) ?? id,
    keys: keymap[id] ?? [],
    custom: id in o,
  }));
}

/** Search by title, command id or shortcut ("ctrl+t", "Ctrl T", "history"). */
export function filterRows(rows: KeymapRow[], query: string): KeymapRow[] {
  // "ctrl+shift+t" is a shortcut: exact match on the keys.
  const chord = query.includes('+') ? parseChord(query) : null;
  if (chord) {
    const want = chordId(chord);
    return rows.filter((r) => r.keys.some((k) => {
      const c = parseChord(k);
      return !!c && chordId(c) === want;
    }));
  }
  const words = query.toLowerCase().split(/[\s+]+/).filter(Boolean);
  if (!words.length) return rows;
  return rows.filter((r) => {
    const hay = `${r.title} ${r.english} ${r.id} ${r.keys.join(' ')}`.toLowerCase().replace(/\+/g, ' ');
    return words.every((w) => hay.includes(w));
  });
}
