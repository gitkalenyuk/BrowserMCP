// Settings → Search, and the address bar's "Search <engine>" hint. Conventions: ../index.svelte.ts.
export default {
  'settings.nav.search': 'Search',
  'settings.search.title': 'Search',
  'settings.search.intro':
    'The search engine the address bar uses when what you type is not an address. Saved in data/config/search.json; agents get the same results URL with search_url.',
  'settings.search.loadError': 'Could not read the search engines: {error}',
  'settings.search.retry': 'Retry',

  'settings.search.engines': 'Search engines',
  'settings.search.enginesLead':
    'Pick the default. Keywords search with another engine without changing it: type the keyword, a space and your words, for example “@ddg weather Kyiv” for DuckDuckGo.',
  'settings.search.col.default': 'Default',
  'settings.search.col.name': 'Engine',
  'settings.search.col.keyword': 'Keyword',
  'settings.search.col.url': 'Search URL',
  'settings.search.col.actions': 'Actions',
  'settings.search.makeDefault': 'Make {name} the default',
  'settings.search.defaultSet': '{name} is now the default search engine',
  'settings.search.defaultChip': 'default',
  'settings.search.customChip': 'added',
  'settings.search.edit': 'Edit {name}',
  'settings.search.remove': 'Remove {name}',
  'settings.search.removed': 'Removed {name}',

  'settings.search.addTitle': 'Add a search engine',
  'settings.search.editTitle': 'Edit the search engine',
  'settings.search.addLead':
    'Search for something on the site, copy the address of the results page and put %s where your words are, e.g. https://uk.wikipedia.org/w/index.php?search=%s.',
  'settings.search.field.name': 'Name',
  'settings.search.field.namePlaceholder': 'Wikipedia',
  'settings.search.field.url': 'Search URL',
  'settings.search.field.urlHelp': 'Starts with https:// and has %s where the words go.',
  'settings.search.field.keyword': 'Keyword',
  'settings.search.field.keywordHelp': 'Optional, no spaces. Empty: made from the name.',
  'settings.search.addButton': 'Add engine',
  'settings.search.added': 'Added {name}',
  'settings.search.updated': 'Saved {name}',

  'settings.search.err.name.required': 'Give the engine a name.',
  'settings.search.err.name.tooLong': 'At most 60 characters.',
  'settings.search.err.url.required': 'Enter the search URL.',
  'settings.search.err.url.scheme': 'The URL must start with https:// or http://.',
  'settings.search.err.url.spaces': 'The URL must not contain spaces.',
  'settings.search.err.url.placeholder': 'Put %s where the search words go.',
  'settings.search.err.keyword.spaces': 'The keyword must not contain spaces or %.',
  'settings.search.err.keyword.tooLong': 'At most 20 characters.',
  'settings.search.err.keyword.taken': 'Another engine already uses this keyword.',

  'settings.search.tryTitle': 'Try it',
  'settings.search.tryLabel': 'Type as in the address bar',
  'settings.search.tryHelp': 'Shows what Enter would open, with the settings above.',
  'settings.search.tryPlaceholder': 'weather Kyiv',
  'settings.search.trySearch': 'Searches with {engine}:',
  'settings.search.tryOpen': 'Opens the address:',

  'settings.search.omniboxSearch': 'Search {engine}',
  'settings.search.omniboxOpen': 'Open',
};
