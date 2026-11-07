// Settings → General → Language: website languages, browser language, spellcheck
// (settings/WebLanguages.svelte). Conventions: ../index.svelte.ts.
export default {
  'languages.web': 'Languages for websites',
  'languages.webHelp':
    'Websites see these languages, most preferred first (the Accept-Language header and navigator.languages), and pick the version of the page they show from them. Changes apply right away to every tab; reload a page to see it in the new language.',
  'languages.automatic': 'automatic',
  'languages.automaticHint': 'The interface language, then English',
  'languages.useAutomatic': 'Use automatic',
  'languages.automaticBack': 'Websites get the interface language, then English',
  'languages.loadError': 'Could not read the language settings: {error}',
  'languages.saved': 'Website languages saved',
  'languages.spell': 'Spellcheck',
  'languages.spellHint': 'Check spelling in this language in text fields',
  'languages.spellFor': 'Spellcheck in {name}',
  'languages.spellSaved': 'Spellcheck languages saved',
  'languages.spellExtra': 'Spellcheck is also on for:',
  'languages.spellOff': 'Turn off spellcheck in {name}',
  'languages.spellUnsupported': 'Spellcheck cannot be set up in this build of the browser.',
  'languages.moveUp': 'Move {name} up',
  'languages.moveDown': 'Move {name} down',
  'languages.remove': 'Remove {name}',
  'languages.keepOne': 'Websites need at least one language',
  'languages.addPlaceholder': 'Add a language: Deutsch, fr, pt-BR…',
  'languages.addLabel': 'Language to add',
  'languages.err.unknown': '“{text}” is not a language: pick one from the list or type a code like de or pt-BR',
  'languages.err.listed': '{name} is already in the list',
  'languages.precedence':
    'Profiles with a stealth identity keep their own languages, matched to the country of their proxy: a site must see a language that fits the IP address.',
  'languages.ownProfiles': 'Keeping their own now:',
  'languages.browser': 'Browser language',
  'languages.browserHelp':
    'The language of Chromium itself: the page context menu, its dialogs and error pages. Only languages shipped with this browser are listed.',
  'languages.noLocales': 'No Chromium language files were found next to the browser (the locales folder).',
  'languages.system': 'Same as Windows',
  'languages.restart': 'Restart the browser to apply',
  'languages.browserSaved': 'Browser language saved: restart the browser to apply it',
};
