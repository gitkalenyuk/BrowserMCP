import { describe, expect, it } from 'vitest';
import { DEFAULT_KEYMAP } from '../keymap';
import { chordText, conflicts, effectiveKeymap, filterRows, keymapRows, recordChord, resetBinding, setBinding } from './keys';

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
});
