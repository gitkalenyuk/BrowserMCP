// Vitest setup: tests run in English whatever the machine's language is. The UI language
// defaults to `system` (navigator.languages), and Node reports the OS locale there, so a
// Ukrainian Windows would otherwise switch every test to Ukrainian. Tests of the Ukrainian
// strings pick the language explicitly (settings.update({ language: 'uk' })).
//
// Only the language fields are overridden: the rest of `navigator` (clipboard, onLine, …)
// stays whatever the environment provides.
const LANGUAGE = { language: 'en-US', languages: ['en-US'] };

if (typeof navigator === 'undefined') {
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { ...LANGUAGE } });
} else {
  for (const [name, value] of Object.entries(LANGUAGE)) {
    Object.defineProperty(navigator, name, { configurable: true, get: () => value });
  }
}
