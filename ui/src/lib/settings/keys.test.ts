import { describe, expect, it } from 'vitest';
import { DICTS } from '../i18n/index.svelte';
import { COMMAND_IDS, DEFAULT_KEYMAP } from '../keymap';
import { settings } from '../settings.svelte';
import { chordText, conflicts, effectiveKeymap, filterRows, keymapRows, recordChord, resetBinding, setBinding } from './keys';

// Titles are checked in English unless a test says otherwise (the machine's language may differ).
settings.update({ language: 'en' });

const key = (code: string, k: string, mods: Partial<Record<'ctrlKey' | 'shiftKey' | 'altKey' | 'metaKey', boolean>> = {}) => ({
  code,
  key: k,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  metaKey: false,
  ...mods,
});

describe('keymap overrides', () => {
  it('stores only differences from the defaults', () => {
    let o = setBinding({}, 'tab.new', ['Ctrl+T', 'Ctrl+N']);
    expect(o).toEqual({ 'tab.new': ['Ctrl+T', 'Ctrl+N'] });
    expect(effectiveKeymap(o)['tab.new']).toEqual(['Ctrl+T', 'Ctrl+N']);
    // Back to the default (any order / spelling): the override goes away.
    o = setBinding(o, 'tab.new', ['ctrl+t']);
    expect(o).toEqual({});
    o = setBinding(o, 'nav.reload', []);
    expect(effectiveKeymap(o)['nav.reload']).toEqual([]);
    expect(resetBinding(o, 'nav.reload')).toEqual({});
  });

  it('has a Settings shortcut', () => {
    expect(DEFAULT_KEYMAP['settings.open']).toEqual(['Ctrl+,']);
    expect(conflicts(DEFAULT_KEYMAP).size).toBe(0);
  });

  it('records chords from key presses', () => {
    expect(recordChord(key('KeyK', 'k', { ctrlKey: true, shiftKey: true }))).toBe('Ctrl+Shift+K');
    expect(recordChord(key('Comma', ',', { ctrlKey: true }))).toBe('Ctrl+,');
    expect(recordChord(key('F7', 'F7'))).toBe('F7');
    expect(recordChord(key('ArrowLeft', 'ArrowLeft', { altKey: true }))).toBe('Alt+Left');
    expect(recordChord(key('KeyA', 'a'))).toBeNull(); // plain letters type text
    expect(recordChord(key('ShiftLeft', 'Shift', { shiftKey: true }))).toBeNull();
    expect(chordText({ ctrl: true, alt: false, shift: false, meta: false, key: '`' })).toBe('Ctrl+`');
  });

  it('finds conflicts', () => {
    const km = effectiveKeymap({ 'tab.new': ['Ctrl+T', 'Ctrl+H'] });
    expect([...conflicts(km)]).toEqual([['ctrl+h', ['tab.new', 'library.history']]]);
  });

  it('searches by title, id and keys', () => {
    const rows = keymapRows(DEFAULT_KEYMAP, {});
    expect(filterRows(rows, 'history').map((r) => r.id)).toContain('library.history');
    expect(filterRows(rows, 'ctrl+shift+t').map((r) => r.id)).toEqual(['tab.reopen']);
    expect(filterRows(rows, 'Ctrl ,').map((r) => r.id)).toEqual(['settings.open']);
    expect(filterRows(rows, '').length).toBe(rows.length);
    expect(rows.find((r) => r.id === 'settings.open')?.title).toBe('Settings');
  });

  it('shows titles in the interface language and searches both languages', () => {
    settings.update({ language: 'uk' });
    try {
      const rows = keymapRows(DEFAULT_KEYMAP, {});
      expect(rows.find((r) => r.id === 'library.history')?.title).toBe('Історія');
      expect(rows.find((r) => r.id === 'tab.select3')?.title).toBe('Перейти до вкладки 3');
      expect(filterRows(rows, 'історія').map((r) => r.id)).toContain('library.history');
      expect(filterRows(rows, 'history').map((r) => r.id)).toContain('library.history');
    } finally {
      settings.update({ language: 'en' });
    }
  });

  it('has a translation for every command', () => {
    const missing = COMMAND_IDS.filter((id) => {
      const key = /^tab\.select\d$/.test(id) ? 'settings.keyboard.cmd.tab.selectN' : `settings.keyboard.cmd.${id}`;
      return !(key in DICTS.en) || !(key in DICTS.uk);
    });
    expect(missing).toEqual([]);
  });
});
