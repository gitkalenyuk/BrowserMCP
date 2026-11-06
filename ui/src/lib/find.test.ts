import { describe, expect, it } from 'vitest';
import { findArgs, findLabel } from './find';
import { buildKeyIndex, commandForEvent, DEFAULT_KEYMAP } from './keymap';

describe('find in page', () => {
  it('labels the counter', () => {
    expect(findLabel('', 0, 0)).toBe('');
    expect(findLabel('foo', 0, 0)).toBe('No results');
    expect(findLabel('foo', 12, 3)).toBe('3/12');
  });

  it('maps steps to page.find_text arguments', () => {
    expect(findArgs(3, 'foo', 0)).toEqual({ tab: 3, query: 'foo', next: false, prev: false });
    expect(findArgs(3, 'foo', 1)).toEqual({ tab: 3, query: 'foo', next: true, prev: false });
    expect(findArgs(3, 'foo', -1)).toEqual({ tab: 3, query: 'foo', next: false, prev: true });
  });

  it('opens with Ctrl+F on any layout', () => {
    const index = buildKeyIndex(DEFAULT_KEYMAP);
    const press = { key: 'а', code: 'KeyF', ctrlKey: true, shiftKey: false, altKey: false, metaKey: false };
    expect(commandForEvent(index, press)).toBe('find.open');
  });
});
