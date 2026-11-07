// The guide in every UI language. Both are bundled (a few hundred kB of text): the settings
// page is local, and switching the language must not wait for a download.
import type { Locale } from '../../../lib/i18n/index.svelte';
import type { Chapter } from '../model';
import en from './en';
import uk from './uk';

export const GUIDE: Record<Locale, Chapter[]> = { en, uk };
