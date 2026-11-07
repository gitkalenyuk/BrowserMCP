// Find in page (Ctrl+F): small pure helpers for FindBar.svelte. The search itself runs in
// the core (`page.find_text` / `page.find_clear`, crates/pagetools).
import { t } from './i18n/index.svelte';

/** Which way Enter / Shift+Enter / the arrow buttons move. */
export type FindStep = -1 | 0 | 1;

/** Arguments for `page.find_text`: step 0 = (re)search, 1 = next, -1 = previous. */
export function findArgs(tab: number, query: string, step: FindStep) {
  return { tab, query, next: step === 1, prev: step === -1 };
}

/** Counter text next to the field: "" (nothing typed), "No results", "3/12". */
export function findLabel(query: string, count: number, index: number): string {
  if (!query) return '';
  if (count === 0) return t('components.find.none');
  return `${index}/${count}`;
}
