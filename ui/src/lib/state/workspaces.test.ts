import { describe, expect, it } from 'vitest';
import type { ProfileEntry, TabInfo } from '../api/types';
import {
  askAiPrompt,
  permissionText,
  profileIdFor,
  proxyLabel,
  splitLeftWidth,
  splitRatioAt,
  tabForWorkspace,
  visibleTabs,
  workspacesFromProfiles,
} from './workspaces';

function tab(id: number, extra: Partial<TabInfo> = {}): TabInfo {
  return {
    id,
    title: `T${id}`,
    url: `https://${id}.test/`,
    loading: false,
    favicon: null,
    canGoBack: false,
    canGoForward: false,
    groupId: null,
    lockedBy: null,
    ...extra,
  };
}

function profile(id: string, extra: Partial<ProfileEntry> = {}): ProfileEntry {
  return {
    id,
    label: id,
    color: null,
    proxy: null,
    proxy_server: null,
    accept_language: null,
    identity: null,
    stored: true,
    open: false,
    incognito: false,
    tabs: [],
    launch: false,
    ...extra,
  };
}

describe('workspaces', () => {
  it('come from profile.list: launch first, labels, proxy labels, no incognito', () => {
    const ws = workspacesFromProfiles([
      profile('work', { label: 'Work', color: 'purple', proxy: 'socks5://u:***@1.2.3.4:1080' }),
      profile('default', { launch: true }),
      profile('run-1-r0-a1', { incognito: true }),
      profile('alpha', { label: 'Alpha', identity: 'idw' }),
    ]);
    expect(ws.map((w) => w.id)).toEqual(['default', 'alpha', 'work']);
    expect(ws[2]).toMatchObject({ name: 'Work', color: 'purple', proxy: 'socks5 · 1.2.3.4:1080' });
    expect(ws[1].identity).toBe('idw');
    expect(ws[0].launch).toBe(true);
    expect(proxyLabel(null)).toBe('Direct');
    expect(proxyLabel('host:1')).toBe('host:1');
  });

  it('filter the strip by window, workspace and off-screen flag', () => {
    const state = {
      tabs: [
        tab(1, { window: 5 }), // launch profile (no profile field)
        tab(2, { window: 5, profile: 'work' }),
        tab(3, { window: 9, profile: 'default' }), // other window
        tab(4, { offscreen: true, profile: 'default' }), // automation
        tab(5, { window: 5, profile: 'default' }),
      ],
      groups: [],
      activeId: 2,
    };
    const home = visibleTabs(state, { window: 5, workspace: 'default', launch: 'default' });
    expect(home.tabs.map((t) => t.id)).toEqual([1, 5]);
    expect(home.activeId).toBeNull(); // the active tab is in another workspace
    const work = visibleTabs(state, { window: 5, workspace: 'work', launch: 'default' });
    expect(work.tabs.map((t) => t.id)).toEqual([2]);
    expect(work.activeId).toBe(2);
    // Before profiles are known: every tab of the window (still no off-screen ones).
    const all = visibleTabs(state, { window: null, workspace: null, launch: 'default' });
    expect(all.tabs.map((t) => t.id)).toEqual([1, 2, 3, 5]);
  });

  it('switching picks the last shown tab of the workspace, else its first', () => {
    const tabs = [tab(1, { window: 1 }), tab(2, { window: 1, profile: 'w' }), tab(3, { window: 1, profile: 'w' })];
    const f = { window: 1, workspace: 'w', launch: 'default' };
    expect(tabForWorkspace(tabs, f, {})).toBe(2);
    expect(tabForWorkspace(tabs, f, { w: 3 })).toBe(3);
    expect(tabForWorkspace(tabs, f, { w: 99 })).toBe(2);
    expect(tabForWorkspace(tabs, { ...f, workspace: 'empty' }, {})).toBeNull();
  });

  it('make profile ids from names', () => {
    expect(profileIdFor('My Work 2', [])).toBe('my-work-2');
    expect(profileIdFor('Робота', [])).toBe('workspace');
    expect(profileIdFor('work', ['work', 'work-2'])).toBe('work-3');
  });

  it('split divider math matches the core', () => {
    // Same numbers as protocol::split_bounds (1206 px slot, 6 px gap).
    expect(splitLeftWidth(1206, 0.5)).toBe(600);
    expect(splitLeftWidth(1006, 0.3)).toBe(300);
    expect(splitRatioAt(603, 1206)).toBeCloseTo(0.5);
    expect(splitRatioAt(0, 1206)).toBe(0.15);
    expect(splitRatioAt(5000, 1206)).toBe(0.85);
  });

  it('texts for the AI panel and the permission bar', () => {
    expect(askAiPrompt({ kind: 'selection', text: 'a\nb', url: 'https://p.test/', title: 'P' })).toBe(
      'About this text from "P" (https://p.test/):\n\n> a\n> b\n\n',
    );
    expect(askAiPrompt({ kind: 'page', text: '', url: 'https://p.test/', title: '' })).toBe(
      'About the page https://p.test/:\n\n',
    );
    expect(permissionText(['geolocation'])).toBe('know your location');
    expect(permissionText(['camera', 'microphone'])).toBe('use your camera and use your microphone');
    expect(permissionText(['window_management'])).toBe('window management');
  });
});
