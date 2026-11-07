// Fake browser.languages_get / browser.languages_set for MockApi (`pnpm dev`, tests). Mirrors
// crates/languages: tags are normalized and checked, [] = the automatic list, the browser
// locale must be one of the shipped paks and needs a restart. No browser is touched.

import { RpcError } from './client';
import type { LanguagesInfo, LanguagesSetArgs } from './types-languages';
import { automaticWeb, normalizeTag } from '../settings/languages';

const LOCALES = ['en-US', 'uk'];

function normalizeList(tags: string[]): string[] {
  const out: string[] = [];
  for (const tag of tags) {
    const n = normalizeTag(tag);
    if (!n) throw new RpcError('INVALID_PARAMS', `"${tag}" is not a language tag (use codes like uk, en-US, pt-BR)`);
    if (!out.some((t) => t.toLowerCase() === n.toLowerCase())) out.push(n);
  }
  return out;
}

export function createLanguageMocks() {
  const state = {
    web: [] as string[],
    browserLocale: null as string | null,
    spellcheck: null as string[] | null,
    uiLanguage: null as string | null,
  };
  const started: string | null = null;

  const info = (): LanguagesInfo => ({
    web: state.web.length ? [...state.web] : automaticWeb(state.uiLanguage ?? 'en'),
    web_automatic: state.web.length === 0,
    browser_locale: state.browserLocale,
    browser_locale_active: started,
    restart_required: state.browserLocale !== started,
    browser_locales: [...LOCALES],
    spellcheck: state.spellcheck ? [...state.spellcheck] : ['en-US'],
    spellcheck_default: state.spellcheck === null,
    spellcheck_supported: true,
    ui_language: state.uiLanguage,
    // A stealth profile whose identity was fitted to a German proxy keeps its own language.
    own_language: [{ profile: 'de-proxy', accept_language: 'de-DE,de,en-US,en' }],
  });

  return {
    'browser.languages_get': () => info(),
    'browser.languages_set': (args: LanguagesSetArgs) => {
      const next = { ...state };
      if (args.web) next.web = normalizeList(args.web);
      if (args.spellcheck) next.spellcheck = normalizeList(args.spellcheck);
      if (args.ui_language) {
        const ui = normalizeTag(args.ui_language);
        if (!ui) throw new RpcError('INVALID_PARAMS', `ui_language "${args.ui_language}" is not a language tag`);
        next.uiLanguage = ui;
      }
      if (args.browser_locale) {
        const wanted = args.browser_locale.trim();
        if (wanted.toLowerCase() === 'system') next.browserLocale = null;
        else {
          const found = LOCALES.find((l) => l.toLowerCase() === wanted.toLowerCase());
          if (!found) throw new RpcError('INVALID_PARAMS', `locale "${wanted}" is not shipped with this browser; available: ${LOCALES.join(', ')}`);
          next.browserLocale = found;
        }
      }
      Object.assign(state, next);
      return info();
    },
  };
}
