import { describe, expect, it } from 'vitest';
import type { TabInfo } from '../api/types';
import { activeTab, cycleTab, emptyTabs, groupRuns, reduceTabs, tabByNumber, type TabsState } from './tabs';

const tab = (id: number, extra: Partial<TabInfo> = {}): TabInfo => ({
  id,
  title: `Tab ${id}`,
  url: `https://example.com/${id}`,
  loading: false,
  favicon: null,
  canGoBack: false,
  canGoForward: false,
  groupId: null,
  lockedBy: null,
  ...extra,
});

const state = (n: number, activeId: number | null = 1): TabsState => ({
  tabs: Array.from({ length: n }, (_, i) => tab(i + 1)),
  groups: [],
  activeId,
});

describe('reduceTabs', () => {
  it('resets from a snapshot', () => {
    const s = reduceTabs(emptyTabs, { type: 'reset', snapshot: { tabs: [tab(5)], groups: [], activeId: 5 } });
    expect(activeTab(s)?.id).toBe(5);
  });

  it('inserts created tabs at the given index (clamped) and ignores duplicates', () => {
    let s = reduceTabs(state(2), { type: 'created', payload: { tab: tab(9), index: 1 } });
    expect(s.tabs.map((t) => t.id)).toEqual([1, 9, 2]);
    s = reduceTabs(s, { type: 'created', payload: { tab: tab(10), index: 99 } });
    expect(s.tabs.map((t) => t.id)).toEqual([1, 9, 2, 10]);
    expect(reduceTabs(s, { type: 'created', payload: { tab: tab(9), index: 0 } })).toBe(s);
  });

  it('patches updated fields only', () => {
    const s0 = state(2);
    const s = reduceTabs(s0, { type: 'updated', payload: { id: 2, loading: true, title: 'Loading' } });
    expect(s.tabs[1]).toMatchObject({ id: 2, loading: true, title: 'Loading', url: 'https://example.com/2' });
    expect(s.tabs[0]).toBe(s0.tabs[0]); // untouched tabs keep identity
    expect(s0.tabs[1].loading).toBe(false); // no mutation
  });

  it('returns the same state for unknown tabs', () => {
    const s0 = state(2);
    expect(reduceTabs(s0, { type: 'updated', payload: { id: 42, title: 'x' } })).toBe(s0);
    expect(reduceTabs(s0, { type: 'closed', payload: { id: 42 } })).toBe(s0);
    expect(reduceTabs(s0, { type: 'activated', payload: { id: 42 } })).toBe(s0);
  });

  it('closing the active tab activates the right neighbour, else the left', () => {
    expect(reduceTabs(state(3, 2), { type: 'closed', payload: { id: 2 } }).activeId).toBe(3);
    expect(reduceTabs(state(3, 3), { type: 'closed', payload: { id: 3 } }).activeId).toBe(2);
    expect(reduceTabs(state(3, 1), { type: 'closed', payload: { id: 3 } }).activeId).toBe(1);
    expect(reduceTabs(state(1, 1), { type: 'closed', payload: { id: 1 } }).activeId).toBeNull();
  });
});

describe('tab selection helpers', () => {
  it('cycles with wrap-around', () => {
    expect(cycleTab(state(3, 3), 1)).toBe(1);
    expect(cycleTab(state(3, 1), -1)).toBe(3);
    expect(cycleTab(emptyTabs, 1)).toBeNull();
  });

  it('Ctrl+1..8 pick by position, Ctrl+9 picks the last tab', () => {
    expect(tabByNumber(state(12), 3)).toBe(3);
    expect(tabByNumber(state(12), 9)).toBe(12);
    expect(tabByNumber(state(2), 5)).toBeNull();
    expect(tabByNumber(state(2), 9)).toBe(2);
  });
});

describe('groupRuns', () => {
  it('splits consecutive tabs by group', () => {
    const s: TabsState = {
      tabs: [tab(1), tab(2, { groupId: 'a' }), tab(3, { groupId: 'a' }), tab(4), tab(5, { groupId: 'b' })],
      groups: [
        { id: 'a', name: 'Claude', color: 'orange', agentId: 'claude' },
        { id: 'b', name: 'Codex', color: 'green', agentId: 'codex' },
      ],
      activeId: 1,
    };
    expect(groupRuns(s).map((r) => [r.group?.id ?? null, r.tabs.map((t) => t.id)])).toEqual([
      [null, [1]],
      ['a', [2, 3]],
      [null, [4]],
      ['b', [5]],
    ]);
  });
});
