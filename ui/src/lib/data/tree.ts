// The bookmark tree for the library page and the bookmarks bar.
//
// The core lists one folder at a time (`data.bookmarks {folder}` -> its bookmarks and its
// sub-folders, each with a `position`; folders and bookmarks share one order). `loadTree` walks
// the folders level by level, `buildTree` turns the listings into nested nodes (pure, tested).

import type { BrowserApi } from '../api/client';
import { t } from '../i18n/index.svelte';
import {
  BOOKMARKS_BAR,
  OTHER_BOOKMARKS,
  type DataBookmark,
  type DataBookmarksOutput,
  type DataFolder,
} from '../api/types-data';

export const ROOT_IDS = [BOOKMARKS_BAR, OTHER_BOOKMARKS] as const;
/** Titles of the root folders, in the UI language (getters). */
export const ROOT_TITLES: Record<number, string> = {
  get [BOOKMARKS_BAR]() {
    return t('components.bookmarksBar');
  },
  get [OTHER_BOOKMARKS]() {
    return t('components.bookmarksBar.other');
  },
};

export interface BookmarkNode {
  kind: 'url';
  id: number;
  title: string;
  url: string;
  parent: number;
  position: number;
}

export interface FolderNode {
  kind: 'folder';
  id: number;
  title: string;
  /** null for the two roots. */
  parent: number | null;
  position: number;
  children: TreeNode[];
  /** `children` holds what the core reported (false when the folder was not listed). */
  loaded: boolean;
}

export type TreeNode = BookmarkNode | FolderNode;

/** The folder's direct children in their shared order. */
export function mergeChildren(listing: DataBookmarksOutput): (DataBookmark | DataFolder)[] {
  return [...listing.bookmarks, ...listing.folders].sort((a, b) => a.position - b.position || a.id - b.id);
}

function isFolder(item: DataBookmark | DataFolder): item is DataFolder {
  return !('url' in item);
}

/**
 * Nest the per-folder listings under the given roots. Folders without a listing get
 * `loaded: false` and no children; a folder seen twice (a cycle in bad data) is not expanded again.
 */
export function buildTree(
  listings: ReadonlyMap<number, DataBookmarksOutput>,
  roots: readonly number[] = ROOT_IDS,
  rootTitles: Record<number, string> = ROOT_TITLES,
): FolderNode[] {
  const seen = new Set<number>();
  const folder = (id: number, title: string, parent: number | null, position: number): FolderNode => {
    const listing = seen.has(id) ? undefined : listings.get(id);
    seen.add(id);
    const node: FolderNode = { kind: 'folder', id, title, parent, position, children: [], loaded: !!listing };
    if (!listing) return node;
    node.children = mergeChildren(listing).map((item) =>
      isFolder(item)
        ? folder(item.id, item.title, id, item.position)
        : { kind: 'url', id: item.id, title: item.title, url: item.url, parent: id, position: item.position },
    );
    return node;
  };
  return roots.map((id, i) => folder(id, rootTitles[id] ?? t('components.folderN', { id }), null, i));
}

/** Fetch every folder below the roots (one `data.bookmarks` per folder, a level at a time). */
export async function loadListings(
  api: Pick<BrowserApi, 'call'>,
  roots: readonly number[] = ROOT_IDS,
  maxFolders = 2000,
): Promise<Map<number, DataBookmarksOutput>> {
  const listings = new Map<number, DataBookmarksOutput>();
  let level = [...roots];
  while (level.length && listings.size < maxFolders) {
    const results = await Promise.all(level.map((folder) => api.call('data.bookmarks', { folder, limit: 5000 })));
    const next: number[] = [];
    for (const r of results) {
      if (listings.has(r.folder)) continue;
      listings.set(r.folder, r);
      for (const f of r.folders) if (!listings.has(f.id)) next.push(f.id);
    }
    level = next;
  }
  return listings;
}

export async function loadTree(api: Pick<BrowserApi, 'call'>): Promise<FolderNode[]> {
  return buildTree(await loadListings(api));
}

/** Depth-first search for a folder. */
export function findFolder(roots: readonly FolderNode[], id: number): FolderNode | null {
  for (const r of roots) {
    if (r.id === id) return r;
    const sub = findFolder(r.children.filter((c): c is FolderNode => c.kind === 'folder'), id);
    if (sub) return sub;
  }
  return null;
}

/** Folders from a root down to `id` (inclusive); empty when not found. */
export function folderPath(roots: readonly FolderNode[], id: number): FolderNode[] {
  for (const r of roots) {
    if (r.id === id) return [r];
    const sub = folderPath(r.children.filter((c): c is FolderNode => c.kind === 'folder'), id);
    if (sub.length) return [r, ...sub];
  }
  return [];
}

/** True when `id` is `ancestor` or lies anywhere below it. */
export function isInside(roots: readonly FolderNode[], ancestor: number, id: number): boolean {
  if (ancestor === id) return true;
  const a = findFolder(roots, ancestor);
  return !!a && !!findFolder(a.children.filter((c): c is FolderNode => c.kind === 'folder'), id);
}

/** Every folder, depth-first, with its depth (for "move to" pickers). */
export function flattenFolders(roots: readonly FolderNode[], depth = 0): { node: FolderNode; depth: number }[] {
  return roots.flatMap((r) => [
    { node: r, depth },
    ...flattenFolders(r.children.filter((c): c is FolderNode => c.kind === 'folder'), depth + 1),
  ]);
}

/**
 * `bookmarks.move` position for dropping `dragged` before (or after) `target` among `siblings`
 * (ids in display order). The core counts the index without the moved node.
 */
export function dropPosition(siblings: readonly number[], dragged: number, target: number, after: boolean): number {
  const rest = siblings.filter((id) => id !== dragged);
  const i = rest.indexOf(target);
  if (i < 0) return rest.length;
  return after ? i + 1 : i;
}

/** Can `dragged` go into `folder`? Not into itself or one of its own sub-folders, not the roots. */
export function canDropInto(roots: readonly FolderNode[], dragged: number, folder: number): boolean {
  if ((ROOT_IDS as readonly number[]).includes(dragged)) return false;
  return !isInside(roots, dragged, folder);
}
