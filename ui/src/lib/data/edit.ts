// Edits the core has no single command for (yet): renaming / re-pointing a bookmark, renaming a
// folder, changing a login's site or username. They are composed from add + move + remove (or
// save + delete), keeping the node's place. Note: the edited node gets a new id.

import type { BrowserApi } from '../api/client';
import type { DataBookmark, PasswordEntry } from '../api/types-data';

type Api = Pick<BrowserApi, 'call'>;

/** Replace bookmark `b` by one with the new title/url at the same place. */
export async function editBookmark(api: Api, b: { id: number; folder: number; position: number }, title: string, url: string): Promise<DataBookmark> {
  const created = await api.call('bookmarks.add', { folder: b.folder, title, url });
  await api.call('bookmarks.move', { id: created.id, folder: b.folder, position: b.position });
  await api.call('bookmarks.remove', { id: b.id });
  return created;
}

/** Rename a folder: a new folder at the same place takes over the children in order. */
export async function renameFolder(api: Api, f: { id: number; parent: number; position: number }, title: string): Promise<number> {
  const created = await api.call('bookmarks.add_folder', { parent: f.parent, title });
  const listing = await api.call('data.bookmarks', { folder: f.id, limit: 5000 });
  const children = [...listing.bookmarks, ...listing.folders].sort((a, b) => a.position - b.position || a.id - b.id);
  for (const c of children) await api.call('bookmarks.move', { id: c.id, folder: created.id });
  await api.call('bookmarks.move', { id: created.id, folder: f.parent, position: f.position });
  await api.call('bookmarks.remove', { id: f.id });
  return created.id;
}

/**
 * Save an edited login. An empty `password` keeps the current one (read with reveal). Changing
 * the site or username saves a new login and deletes the old one.
 */
export async function editPassword(
  api: Api,
  old: PasswordEntry,
  next: { origin: string; username: string; password: string; note: string },
): Promise<void> {
  let password = next.password;
  if (!password) {
    const { entries } = await api.call('passwords.list', { origin: old.origin, reveal: true });
    password = entries.find((e) => e.id === old.id)?.password ?? '';
    if (!password) throw new Error('The current password could not be read; enter a new one.');
  }
  const saved = await api.call('passwords.save', { origin: next.origin, username: next.username, password, note: next.note });
  if (saved.id !== old.id) await api.call('passwords.delete', { id: old.id });
}
