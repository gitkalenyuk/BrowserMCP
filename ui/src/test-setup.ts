// Vitest setup: tests run in English whatever the machine's language is. The UI language
// defaults to `system` (navigator.languages), and Node reports the OS locale there, so a
// Ukrainian Windows would otherwise switch every test to Ukrainian. Tests of the Ukrainian
// strings pick the language explicitly (settings.update({ language: 'uk' })).
const nav = globalThis.navigator as Navigator | undefined;
Object.defineProperty(globalThis, 'navigator', {
  configurable: true,
  value: {
    hardwareConcurrency: nav?.hardwareConcurrency ?? 1,
    platform: nav?.platform ?? '',
    userAgent: nav?.userAgent ?? '',
    language: 'en-US',
    languages: ['en-US'],
  },
});
