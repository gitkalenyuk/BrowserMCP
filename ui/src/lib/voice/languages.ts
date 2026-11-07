// Languages Whisper recognizes (ISO 639-1 codes, plus `haw` / `yue`), most used first; names
// come from Intl.DisplayNames in the UI language.

export const VOICE_LANGUAGES = [
  'uk', 'en', 'de', 'fr', 'es', 'it', 'pl', 'pt', 'nl', 'cs', 'sk', 'ro', 'hu', 'bg', 'be', 'ru',
  'tr', 'el', 'he', 'ar', 'fa', 'hi', 'bn', 'ur', 'ja', 'zh', 'yue', 'ko', 'vi', 'th', 'id', 'ms',
  'tl', 'sv', 'no', 'nn', 'da', 'fi', 'is', 'et', 'lv', 'lt', 'sl', 'hr', 'sr', 'bs', 'mk', 'sq',
  'ka', 'hy', 'az', 'kk', 'uz', 'tk', 'tg', 'mn', 'ky', 'ta', 'te', 'kn', 'ml', 'mr', 'gu', 'pa',
  'ne', 'si', 'my', 'km', 'lo', 'sw', 'yo', 'ha', 'so', 'am', 'af', 'ca', 'eu', 'gl', 'cy', 'ga',
  'mt', 'lb', 'fo', 'br', 'oc', 'la', 'eo', 'haw', 'mi', 'jw', 'su', 'ps', 'sd', 'as', 'yi', 'ht',
  'ln', 'mg', 'sn', 'tt', 'ba', 'bo', 'sa',
] as const;

/** "uk" → "Ukrainian" / "українська" in `uiLocale`; the code itself when unknown. */
export function languageName(code: string, uiLocale: string): string {
  try {
    const name = new Intl.DisplayNames([uiLocale], { type: 'language' }).of(code);
    if (!name || name === code) return code;
    return name.charAt(0).toLocaleUpperCase(uiLocale) + name.slice(1);
  } catch {
    return code;
  }
}
