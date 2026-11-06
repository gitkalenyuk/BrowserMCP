// Pure tab-state reducer. The core owns the truth; the chrome folds its events into
// this state. No Svelte here so it can be unit-tested directly.
import type { ApiEvents, TabGroup, TabId, TabInfo, TabsSnapshot } from '../api/types';

export interface TabsState {
  tabs: TabInfo[];
  groups: TabGroup[];
  activeId: TabId | null;
}

export type TabsAction =
  | { type: 'reset'; snapshot: TabsSnapshot }
  | { type: 'created'; payload: ApiEvents['tab.created'] }
  | { type: 'updated'; payload: ApiEvents['tab.updated'] }
  | { type: 'closed'; payload: ApiEvents['tab.closed'] }
  | { type: 'activated'; payload: ApiEvents['tab.activated'] };

export const emptyTabs: TabsState = { tabs: [], groups: [], activeId: null };

export function reduceTabs(state: TabsState, action: TabsAction): TabsState {
  switch (action.type) {
    case 'reset':
      return {
        tabs: action.snapshot.tabs,
        groups: action.snapshot.groups,
        activeId: action.snapshot.activeId,
      };
    case 'created': {
      const { tab, index } = action.payload;
      if (state.tabs.some((t) => t.id === tab.id)) return state;
      const at = Math.max(0, Math.min(index, state.tabs.length));
      const tabs = [...state.tabs.slice(0, at), tab, ...state.tabs.slice(at)];
      return { ...state, tabs };
    }
    case 'updated': {
      const { id, ...patch } = action.payload;
      let found = false;
      const tabs = state.tabs.map((t) => {
        if (t.id !== id) return t;
        found = true;
        return { ...t, ...patch };
      });
      return found ? { ...state, tabs } : state;
    }
    case 'closed': {
      const i = state.tabs.findIndex((t) => t.id === action.payload.id);
      if (i < 0) return state;
      const tabs = state.tabs.filter((_, j) => j !== i);
      // Optimistic neighbour pick (Chrome: the tab to the right, else left); the core
      // confirms with tab.activated.
      let activeId = state.activeId;
      if (activeId === action.payload.id) {
        activeId = tabs.length ? tabs[Math.min(i, tabs.length - 1)].id : null;
      }
      return { ...state, tabs, activeId };
    }
    case 'activated':
      if (!state.tabs.some((t) => t.id === action.payload.id)) return state;
      return { ...state, activeId: action.payload.id };
  }
}

export function activeTab(state: TabsState): TabInfo | null {
  return state.tabs.find((t) => t.id === state.activeId) ?? null;
}

/** Ctrl+Tab / Ctrl+Shift+Tab: neighbour id with wrap-around. */
export function cycleTab(state: TabsState, delta: 1 | -1): TabId | null {
  const n = state.tabs.length;
  if (n === 0) return null;
  const i = state.tabs.findIndex((t) => t.id === state.activeId);
  const next = i < 0 ? 0 : (i + delta + n) % n;
  return state.tabs[next].id;
}

/** Ctrl+1..8 select by position; Ctrl+9 always selects the last tab (Chrome behaviour). */
export function tabByNumber(state: TabsState, n: number): TabId | null {
  if (state.tabs.length === 0 || n < 1) return null;
  if (n >= 9) return state.tabs[state.tabs.length - 1].id;
  return state.tabs[n - 1]?.id ?? null;
}

export interface TabRun {
  group: TabGroup | null;
  tabs: TabInfo[];
}

/** Splits tabs into consecutive runs sharing a group, for rendering group labels. */
export function groupRuns(state: TabsState): TabRun[] {
  const byId = new Map(state.groups.map((g) => [g.id, g]));
  const runs: TabRun[] = [];
  for (const tab of state.tabs) {
    const group = (tab.groupId && byId.get(tab.groupId)) || null;
    const last = runs[runs.length - 1];
    if (last && last.group?.id === group?.id) last.tabs.push(tab);
    else runs.push({ group, tabs: [tab] });
  }
  return runs;
}
