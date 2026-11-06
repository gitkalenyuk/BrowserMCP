import { describe, expect, it } from 'vitest';
import { createDataMocks } from '../api/mock-data';
import type { DataBookmarksOutput } from '../api/types-data';
import {
  buildTree,
  canDropInto,
  dropPosition,
  findFolder,
  flattenFolders,
  folderPath,
  isInside,
  loadTree,
  mergeChildren,
  type FolderNode,
} from './tree';

const bm = (id: number, folder: number, position: number, title = `b${id}`) => ({
  id,
  title,
  url: `https://example.com/${id}`,
  folder,
  position,
  created: 0,
});
const fo = (id: number, parent: number, position: number, title = `f${id}`, children = 0) => ({
  id,
  title,
  parent,
  position,
  created: 0,
  children,
});
const listing = (folder: number, bookmarks: ReturnType<typeof bm>[], folders: ReturnType<typeof fo>[]): DataBookmarksOutput => ({
  folder,
  bookmarks,
  folders,
});

describe('mergeChildren', () => {
  it('interleaves folders and bookmarks by their shared position', () => {
    const merged = mergeChildren(listing(1, [bm(10, 1, 0), bm(11, 1, 2)], [fo(20, 1, 1), fo(21, 1, 3)]));
    expect(merged.map((x) => x.id)).toEqual([10, 20, 11, 21]);
  });
});

describe('buildTree', () => {
  const listings = new Map<number, DataBookmarksOutput>([
    [1, listing(1, [bm(10, 1, 1)], [fo(20, 1, 0, 'Rust', 2)])],
    [2, listing(2, [bm(12, 2, 0)], [])],
    [20, listing(20, [bm(30, 20, 1)], [fo(40, 20, 0, 'CEF', 0)])],
    [40, listing(40, [], [])],
  ]);
  const tree = buildTree(listings);

  it('nests folders under the two roots with their titles', () => {
    expect(tree.map((r) => [r.id, r.title, r.parent])).toEqual([
      [1, 'Bookmarks bar', null],
      [2, 'Other bookmarks', null],
    ]);
    expect(tree[0].children.map((c) => c.id)).toEqual([20, 10]);
    const rust = tree[0].children[0] as FolderNode;
    expect(rust.kind).toBe('folder');
    expect(rust.parent).toBe(1);
    expect(rust.children.map((c) => [c.kind, c.id])).toEqual([
      ['folder', 40],
      ['url', 30],
    ]);
    expect((rust.children[1] as { parent: number }).parent).toBe(20);
  });

  it('marks folders that were not listed', () => {
    const partial = buildTree(new Map([[1, listing(1, [], [fo(20, 1, 0)])]]));
    expect((partial[0].children[0] as FolderNode).loaded).toBe(false);
    expect(partial[1].loaded).toBe(false);
  });

  it('does not loop on a folder listed under itself', () => {
    const cyclic = buildTree(new Map([[1, listing(1, [], [fo(1, 1, 0)])]]), [1]);
    expect((cyclic[0].children[0] as FolderNode).children).toEqual([]);
  });

  it('finds folders, paths and descendants', () => {
    expect(findFolder(tree, 40)?.title).toBe('CEF');
    expect(findFolder(tree, 10)).toBeNull();
    expect(folderPath(tree, 40).map((f) => f.title)).toEqual(['Bookmarks bar', 'Rust', 'CEF']);
    expect(folderPath(tree, 999)).toEqual([]);
    expect(isInside(tree, 20, 40)).toBe(true);
    expect(isInside(tree, 40, 20)).toBe(false);
    expect(flattenFolders(tree).map(({ node, depth }) => `${depth}:${node.title}`)).toEqual([
      '0:Bookmarks bar',
      '1:Rust',
      '2:CEF',
      '0:Other bookmarks',
    ]);
  });

  it('refuses drops of a folder into itself or below, and of the roots', () => {
    expect(canDropInto(tree, 20, 40)).toBe(false);
    expect(canDropInto(tree, 20, 20)).toBe(false);
    expect(canDropInto(tree, 20, 2)).toBe(true);
    expect(canDropInto(tree, 10, 40)).toBe(true);
    expect(canDropInto(tree, 1, 2)).toBe(false);
  });
});

describe('dropPosition', () => {
  const sibs = [1, 2, 3, 4];
  it('counts without the dragged node (the core moves it out first)', () => {
    expect(dropPosition(sibs, 1, 3, false)).toBe(1); // [2, *1*, 3, 4]
    expect(dropPosition(sibs, 1, 3, true)).toBe(2); // [2, 3, *1*, 4]
    expect(dropPosition(sibs, 4, 1, false)).toBe(0);
    expect(dropPosition(sibs, 4, 3, true)).toBe(3);
  });
  it('handles nodes coming from another folder and unknown targets', () => {
    expect(dropPosition(sibs, 9, 1, false)).toBe(0);
    expect(dropPosition(sibs, 9, 4, true)).toBe(4);
    expect(dropPosition(sibs, 9, 42, true)).toBe(4);
  });
});

describe('loadTree against the mock core', () => {
  it('walks every folder and keeps moves consistent with the core', async () => {
    const h = createDataMocks({ now: () => 1_700_000_000_000 });
    const api = { call: (m: string, p: unknown) => Promise.resolve((h as Record<string, (p: unknown) => unknown>)[m](p)) } as never;
    let tree = await loadTree(api);
    const bar = tree[0];
    const titles = bar.children.map((c) => c.title);
    expect(titles.slice(0, 3)).toEqual(['GitHub', 'MDN Web Docs', 'Rust']);
    const rust = bar.children[2] as FolderNode;
    expect(rust.children.some((c) => c.kind === 'folder' && c.title === 'CEF')).toBe(true);

    // Drag "GitHub" after "Rust": position from dropPosition, then re-read.
    const github = bar.children[0];
    const position = dropPosition(bar.children.map((c) => c.id), github.id, rust.id, true);
    h['bookmarks.move']({ id: github.id, folder: 1, position });
    tree = await loadTree(api);
    expect(tree[0].children.slice(0, 3).map((c) => c.title)).toEqual(['MDN Web Docs', 'Rust', 'GitHub']);
  });
});
