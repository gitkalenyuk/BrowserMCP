import { describe, expect, it } from 'vitest';
import { createDataMocks } from '../api/mock-data';
import { editBookmark, editPassword, renameFolder } from './edit';
import { formatBytes, fileName, matchesLogin } from './format';
import { LIBRARY_URL, isLibraryUrl, parseRequest } from './library';
import { loadTree, type FolderNode } from './tree';

describe('library request', () => {
  const now = 1_000_000;
  it('accepts fresh, valid requests only', () => {
    expect(parseRequest({ section: 'history', at: now - 1000 }, now)).toEqual({ section: 'history', at: now - 1000 });
    expect(parseRequest({ section: 'bookmarks', folder: 7, at: now }, now)).toEqual({ section: 'bookmarks', folder: 7, at: now });
    expect(parseRequest({ section: 'bookmarks', folder: -1, at: now }, now)).toEqual({ section: 'bookmarks', at: now });
    expect(parseRequest({ section: 'history', at: now - 60_000 }, now)).toBeNull();
    expect(parseRequest({ section: 'evil', at: now }, now)).toBeNull();
    expect(parseRequest('history', now)).toBeNull();
    expect(parseRequest(null, now)).toBeNull();
  });

  it('matches only the exact library URL', () => {
    expect(isLibraryUrl(LIBRARY_URL)).toBe(true);
    expect(isLibraryUrl(`${LIBRARY_URL}?x=1`)).toBe(false);
    expect(isLibraryUrl('https://bmcp/ui/library.html')).toBe(false);
  });
});

const mockApi = () => {
  const h = createDataMocks({ now: () => 1_700_000_000_000 }) as unknown as Record<string, (p: unknown) => unknown>;
  return { call: (m: string, p: unknown) => Promise.resolve().then(() => h[m](p)) } as never;
};

describe('composed edits', () => {
  it('edits a bookmark in place', async () => {
    const api = mockApi();
    const bar = (await loadTree(api))[0];
    const mdn = bar.children[1];
    expect(mdn.title).toBe('MDN Web Docs');
    await editBookmark(api, { id: mdn.id, folder: 1, position: mdn.position }, 'MDN', 'https://developer.mozilla.org/en-US/');
    const after = (await loadTree(api))[0].children;
    expect(after[1]).toMatchObject({ kind: 'url', title: 'MDN', url: 'https://developer.mozilla.org/en-US/' });
    expect(after.length).toBe(bar.children.length);
  });

  it('renames a folder keeping its place and contents', async () => {
    const api = mockApi();
    const bar = (await loadTree(api))[0];
    const rust = bar.children[2] as FolderNode;
    const before = rust.children.map((c) => c.title);
    await renameFolder(api, { id: rust.id, parent: 1, position: rust.position }, 'Rust lang');
    const renamed = (await loadTree(api))[0].children[2] as FolderNode;
    expect(renamed.title).toBe('Rust lang');
    expect(renamed.children.map((c) => c.title)).toEqual(before);
  });

  it('edits a login, keeping the password when none is typed', async () => {
    const api = mockApi() as { call: (m: string, p?: unknown) => Promise<{ entries: { id: number; username: string; password: string | null }[] }> };
    const [gh] = (await api.call('passwords.list', { origin: 'https://github.com' })).entries;
    await editPassword(api as never, gh as never, { origin: 'https://github.com', username: 'octo', password: '', note: 'renamed' });
    const after = (await api.call('passwords.list', { origin: 'https://github.com', reveal: true })).entries;
    expect(after.map((e) => [e.username, e.password])).toEqual([['octo', 'hunter2-gh']]);
  });
});

describe('format', () => {
  it('formats sizes and file names', () => {
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1_204_224)).toBe('1.1 MB');
    expect(formatBytes(4_700_000_000)).toBe('4.4 GB');
    expect(formatBytes(null)).toBe('?');
    expect(fileName('C:\\Users\\you\\Downloads\\a.pdf')).toBe('a.pdf');
    expect(fileName('/tmp/x/b.zip')).toBe('b.zip');
  });
  it('filters logins by site, user or note', () => {
    const e = { origin: 'https://github.com', username: 'octocat', note: 'work' };
    expect(matchesLogin(e, 'GITHUB')).toBe(true);
    expect(matchesLogin(e, 'octo')).toBe(true);
    expect(matchesLogin(e, 'work')).toBe(true);
    expect(matchesLogin(e, 'gitlab')).toBe(false);
    expect(matchesLogin(e, '  ')).toBe(true);
  });
});
