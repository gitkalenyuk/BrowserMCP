// Library edits. Bookmarks and folders are edited in place with `bookmarks.update` (the id and
// position stay). A login's site or username has no update command: it is saved anew and the
// old login deleted.

import type { BrowserApi } from '../api/client';
import type { DataBookmark, DataFolder, PasswordEntry } from '../api/types-data';

type Api = Pick<BrowserApi, 'call'>;

/** Give bookmark `id` a new title and URL (the core accepts http, https and file URLs only). */
export async function editBookmark(api: Api, id: number, title: string, url: string): Promise<DataBookmark | null> {
  return (await api.call('bookmarks.update', { id, title, url })).bookmark;
}

/** Rename folder `id` in place; its contents and position stay. */
export async function renameFolder(api: Api, id: number, title: string): Promise<DataFolder | null> {
  return (await api.call('bookmarks.update', { id, title })).folder;
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
