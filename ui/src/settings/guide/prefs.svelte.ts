// Per-viewer conveniences of the guide (localStorage; storage.ts never throws).

import { load, save } from '../../lib/storage';
import { API_FLAVORS, type ApiFlavor } from './snippets';

const FLAVOR_KEY = 'settings.guide.api';
const CHAPTER_KEY = 'settings.guide.chapter';

function storedFlavor(): ApiFlavor {
  const v = load<unknown>(FLAVOR_KEY, 'mcp');
  return (API_FLAVORS as unknown[]).includes(v) ? (v as ApiFlavor) : 'mcp';
}

/** Which way of calling a command the examples show; shared by every example on the page. */
export const apiPref = $state({ flavor: storedFlavor() });

export function setApiFlavor(flavor: ApiFlavor): void {
  apiPref.flavor = flavor;
  save(FLAVOR_KEY, flavor);
}

/** The chapter the guide showed last (null = the overview). */
export function lastChapter(): string | null {
  const v = load<unknown>(CHAPTER_KEY, null);
  return typeof v === 'string' ? v : null;
}

export function rememberChapter(id: string | null): void {
  save(CHAPTER_KEY, id);
}
