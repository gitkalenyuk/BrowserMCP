import { describe, expect, it } from 'vitest';
import {
  buildKeyIndex,
  chordFromEvent,
  chordId,
  chordLabels,
  commandForEvent,
  COMMAND_IDS,
  DEFAULT_KEYMAP,
  parseChord,
  parseKeymapOverrides,
  type KeyLike,
} from './keymap';

const ev = (p: Partial<KeyLike>): KeyLike => ({
  key: '',
  code: '',
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  metaKey: false,
  ...p,
});

describe('parseChord', () => {
  it('parses modifiers and keys in any case/order', () => {
    expect(parseChord('Ctrl+Shift+Tab')).toEqual({ ctrl: true, shift: true, alt: false, meta: false, key: 'tab' });
    expect(chordId(parseChord('shift+CTRL+t')!)).toBe('ctrl+shift+t');
  });

  it('normalizes aliases', () => {
    expect(parseChord('Alt+Left')?.key).toBe('arrowleft');
    expect(parseChord('Esc')?.key).toBe('escape');
    expect(parseChord('Ctrl++')?.key).toBe('+');
    expect(parseChord('Ctrl+`')?.key).toBe('`');
    expect(parseChord('F5')?.key).toBe('f5');
  });

  it('maps Mod to Ctrl, or Cmd on macOS', () => {
    expect(parseChord('Mod+K')?.ctrl).toBe(true);
    expect(parseChord('Mod+K', { mac: true })?.meta).toBe(true);
  });

  it('rejects garbage', () => {
    expect(parseChord('')).toBeNull();
    expect(parseChord('Ctrl+')).toBeNull();
    expect(parseChord('Hyper+K')).toBeNull();
    expect(parseChord('Ctrl+NotAKey')).toBeNull();
  });
});

describe('chordFromEvent', () => {
  it('uses the physical key so shortcuts work on non-Latin layouts', () => {
    // Ctrl+T on a Ukrainian layout: key is "е", code is still KeyT.
    expect(chordId(chordFromEvent(ev({ key: 'е', code: 'KeyT', ctrlKey: true }))!)).toBe('ctrl+t');
    // Ctrl+Shift+1 reports key "!" — code wins.
    expect(chordId(chordFromEvent(ev({ key: '!', code: 'Digit1', ctrlKey: true, shiftKey: true }))!)).toBe(
      'ctrl+shift+1',
    );
  });

  it('ignores bare modifier presses', () => {
    expect(chordFromEvent(ev({ key: 'Control', code: 'ControlLeft', ctrlKey: true }))).toBeNull();
  });
});

describe('default keymap', () => {
  const index = buildKeyIndex(DEFAULT_KEYMAP);

  it('binds every command id (possibly to nothing) and parses every chord', () => {
    for (const id of COMMAND_IDS) {
      expect(DEFAULT_KEYMAP[id]).toBeDefined();
      for (const c of DEFAULT_KEYMAP[id]) expect(parseChord(c), `${id}: ${c}`).not.toBeNull();
    }
  });

  it('has no conflicting chords', () => {
    const seen = new Map<string, string>();
    for (const id of COMMAND_IDS) {
      for (const c of DEFAULT_KEYMAP[id]) {
        const k = chordId(parseChord(c)!);
        expect(seen.get(k), `${c} bound to ${seen.get(k)} and ${id}`).toBeUndefined();
        seen.set(k, id);
      }
    }
  });

  it('resolves Chrome shortcuts', () => {
    expect(commandForEvent(index, ev({ key: 't', code: 'KeyT', ctrlKey: true }))).toBe('tab.new');
    expect(commandForEvent(index, ev({ key: 'Tab', code: 'Tab', ctrlKey: true, shiftKey: true }))).toBe('tab.prev');
    expect(commandForEvent(index, ev({ key: '9', code: 'Digit9', ctrlKey: true }))).toBe('tab.selectLast');
    expect(commandForEvent(index, ev({ key: 'k', code: 'KeyK', ctrlKey: true }))).toBe('palette.open');
    expect(commandForEvent(index, ev({ key: 'k', code: 'KeyK' }))).toBeNull();
  });

  it('leaves copy & paste keys to the text, even when keymap.json binds them', () => {
    const { keymap } = parseKeymapOverrides({ 'tab.new': ['Ctrl+T', 'Ctrl+C'], 'palette.open': ['Ctrl+K', 'Ctrl+A', 'Shift+Insert'] });
    const custom = buildKeyIndex(keymap);
    // Ukrainian layout: key "с" on the C key.
    expect(commandForEvent(custom, ev({ key: 'с', code: 'KeyC', ctrlKey: true }))).toBeNull();
    expect(commandForEvent(custom, ev({ key: 'a', code: 'KeyA', ctrlKey: true }))).toBeNull();
    expect(commandForEvent(custom, ev({ key: 'Insert', code: 'Insert', shiftKey: true }))).toBeNull();
    for (const code of ['KeyV', 'KeyX', 'KeyZ', 'KeyY']) {
      expect(commandForEvent(index, ev({ key: code.slice(3).toLowerCase(), code, ctrlKey: true }))).toBeNull();
    }
    // The other bindings still work.
    expect(commandForEvent(custom, ev({ key: 't', code: 'KeyT', ctrlKey: true }))).toBe('tab.new');
    expect(commandForEvent(custom, ev({ key: 'k', code: 'KeyK', ctrlKey: true }))).toBe('palette.open');
  });
});

describe('parseKeymapOverrides', () => {
  it('replaces, unbinds and reports problems without dropping valid entries', () => {
    const { keymap, errors } = parseKeymapOverrides({
      'palette.open': ['Ctrl+P', 'Ctrl+Nope'],
      'tab.close': [],
      'nav.reload': 'F5',
      'no.such.command': ['Ctrl+X'],
      'tab.new': 42,
    });
    expect(keymap['palette.open']).toEqual(['Ctrl+P']);
    expect(keymap['tab.close']).toEqual([]);
    expect(keymap['nav.reload']).toEqual(['F5']);
    expect(keymap['tab.new']).toEqual(DEFAULT_KEYMAP['tab.new']);
    expect(errors).toHaveLength(3);
    const index = buildKeyIndex(keymap);
    expect(commandForEvent(index, ev({ key: 'w', code: 'KeyW', ctrlKey: true }))).toBeNull();
  });

  it('rejects non-objects', () => {
    expect(parseKeymapOverrides([]).errors).toHaveLength(1);
    expect(parseKeymapOverrides('x').keymap).toEqual(DEFAULT_KEYMAP);
  });
});

describe('chordLabels', () => {
  it('formats keycaps', () => {
    expect(chordLabels('Ctrl+Shift+Tab')).toEqual(['Ctrl', 'Shift', 'Tab']);
    expect(chordLabels('Alt+Left')).toEqual(['Alt', '←']);
    expect(chordLabels('Ctrl+k')).toEqual(['Ctrl', 'K']);
  });
});
