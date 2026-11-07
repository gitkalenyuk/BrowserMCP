import { describe, expect, it } from 'vitest';
import {
  addLanguage,
  addable,
  automaticWeb,
  moveLanguage,
  nativeName,
  normalizeTag,
  removeLanguage,
  resolveLanguage,
  spellExtras,
  toggleSpellcheck,
} from './languages';

describe('language tags', () => {
  it('normalizes like the Rust side', () => {
    expect(normalizeTag('EN-us')).toBe('en-US');
    expect(normalizeTag(' uk_ua ')).toBe('uk-UA');
    expect(normalizeTag('zh-hant-tw')).toBe('zh-Hant-TW');
    expect(normalizeTag('es-419')).toBe('es-419');
    for (const bad of ['', 'e', 'english', 'en;q=0.9', 'en,de', 'en-', 'en-a']) expect(normalizeTag(bad), bad).toBeNull();
  });

  it('names languages in their own language', () => {
    expect(nativeName('uk')).toBe('Українська');
    expect(nativeName('de')).toBe('Deutsch');
    expect(nativeName('qq-zz-zz')).toBe('qq-zz-zz');
  });

  it('resolves codes and names typed in the add box', () => {
    expect(resolveLanguage('de', 'en')).toBe('de');
    expect(resolveLanguage('Deutsch', 'en')).toBe('de');
    expect(resolveLanguage('german', 'en')).toBe('de');
    expect(resolveLanguage('Німецька', 'uk')).toBe('de');
    expect(resolveLanguage('pt-br', 'en')).toBe('pt-BR');
    expect(resolveLanguage('not a language', 'en')).toBeNull();
    expect(resolveLanguage('  ', 'en')).toBeNull();
  });
});

describe('list editing', () => {
  it('adds without duplicates and keeps at least one language', () => {
    expect(addLanguage(['uk'], 'en')).toEqual(['uk', 'en']);
    expect(addLanguage(['uk', 'en'], 'EN')).toEqual(['uk', 'en']);
    expect(removeLanguage(['uk', 'en'], 0)).toEqual(['en']);
    expect(removeLanguage(['uk'], 0)).toEqual(['uk']);
  });

  it('moves up and down within bounds', () => {
    expect(moveLanguage(['uk', 'en', 'de'], 2, -1)).toEqual(['uk', 'de', 'en']);
    expect(moveLanguage(['uk', 'en', 'de'], 0, 1)).toEqual(['en', 'uk', 'de']);
    expect(moveLanguage(['uk', 'en'], 0, -1)).toEqual(['uk', 'en']);
    expect(moveLanguage(['uk', 'en'], 1, 1)).toEqual(['uk', 'en']);
  });

  it('toggles spellcheck in the order of the website list', () => {
    const web = ['uk', 'en', 'de'];
    expect(toggleSpellcheck([], 'en', web)).toEqual(['en']);
    expect(toggleSpellcheck(['en'], 'uk', web)).toEqual(['uk', 'en']);
    expect(toggleSpellcheck(['uk', 'en'], 'UK', web)).toEqual(['en']);
    expect(toggleSpellcheck(['fr'], 'de', web)).toEqual(['de', 'fr']);
  });

  it('lists dictionaries that have no row in the website list', () => {
    expect(spellExtras(['en-US'], ['uk', 'en'])).toEqual(['en-US']);
    expect(spellExtras(['UK', 'en-US'], ['uk', 'en-US'])).toEqual([]);
    // Turning an extra off is the same toggle as a row's checkbox.
    expect(toggleSpellcheck(['uk', 'en-US'], 'en-US', ['uk', 'en'])).toEqual(['uk']);
  });

  it('offers only languages not in the list', () => {
    const left = addable(['uk', 'EN']);
    expect(left).not.toContain('uk');
    expect(left).not.toContain('en');
    expect(left).toContain('de');
  });

  it('builds the automatic list like the Rust side', () => {
    expect(automaticWeb('uk')).toEqual(['uk', 'en']);
    expect(automaticWeb('uk-UA')).toEqual(['uk-UA', 'uk', 'en']);
    expect(automaticWeb('en')).toEqual(['en-US', 'en']);
  });
});
