import { describe, expect, it } from 'vitest';
import type { UiNotification } from './api/types';
import { MAX_VISIBLE, addToast, expire, removeToast, visibleToasts, type Toast } from './notifications';

function n(id: string, extra: Partial<UiNotification> = {}): UiNotification {
  return { id, title: id, body: '', level: 'info', actions: [], sticky: false, topic: null, ...extra };
}
const question = (id: string) => n(id, { actions: [{ id: 'c0', label: 'Yes' }], sticky: true });

describe('notification toasts', () => {
  it('plain toasts fade out, questions stay', () => {
    let list: Toast[] = [];
    list = addToast(list, n('a'), 1000);
    list = addToast(list, n('b', { level: 'error' }), 1000);
    list = addToast(list, question('q'), 1000);
    expect(list.map((t) => t.id)).toEqual(['q', 'b', 'a']);
    expect(list[0].expiresAt).toBeNull();

    list = expire(list, 1000 + 6001); // info gone, error (12 s) still there
    expect(list.map((t) => t.id)).toEqual(['q', 'b']);
    list = expire(list, 1000 + 60_000);
    expect(list.map((t) => t.id)).toEqual(['q']);
    expect(expire(list, 1e12)).toBe(list); // unchanged → same array
  });

  it('questions stay in front of newer plain toasts; same id replaces', () => {
    let list: Toast[] = [];
    list = addToast(list, question('q'), 0);
    list = addToast(list, n('a'), 0);
    expect(list.map((t) => t.id)).toEqual(['q', 'a']);
    list = addToast(list, n('a', { title: 'updated' }), 0);
    expect(list).toHaveLength(2);
    expect(list[1].title).toBe('updated');
    expect(removeToast(list, 'q').map((t) => t.id)).toEqual(['a']);
  });

  it('shows at most MAX_VISIBLE', () => {
    let list: Toast[] = [];
    for (let i = 0; i < MAX_VISIBLE + 2; i++) list = addToast(list, n(`n${i}`), 0);
    const { shown, hidden } = visibleToasts(list);
    expect(shown).toHaveLength(MAX_VISIBLE);
    expect(hidden).toBe(2);
  });
});
