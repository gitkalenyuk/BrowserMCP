// Налаштування → Пошук і підказка «Шукати (…)» в адресному рядку. Правила: ../index.svelte.ts.
export default {
  'settings.search.nav': 'Пошук',
  'settings.search.title': 'Пошук',
  'settings.search.intro':
    'Пошукова система, яку використовує адресний рядок, коли введене не схоже на адресу. Зберігається в data/config/search.json; агенти отримують таку саму адресу результатів через search_url.',
  'settings.search.loadError': 'Не вдалося прочитати пошукові системи: {error}',
  'settings.search.retry': 'Спробувати ще раз',

  'settings.search.engines': 'Пошукові системи',
  'settings.search.enginesLead':
    'Виберіть систему за замовчуванням. Ключові слова дають змогу шукати в іншій системі, не змінюючи її: введіть ключове слово, пробіл і запит, наприклад «d погода Київ» для DuckDuckGo.',
  'settings.search.col.default': 'Основна',
  'settings.search.col.name': 'Система',
  'settings.search.col.keyword': 'Ключове слово',
  'settings.search.col.url': 'Адреса пошуку',
  'settings.search.col.actions': 'Дії',
  'settings.search.makeDefault': 'Зробити {name} основною',
  'settings.search.defaultSet': '{name} тепер основна пошукова система',
  'settings.search.defaultChip': 'основна',
  'settings.search.customChip': 'додана',
  'settings.search.edit': 'Змінити {name}',
  'settings.search.remove': 'Видалити {name}',
  'settings.search.removed': '{name} видалено',

  'settings.search.addTitle': 'Додати пошукову систему',
  'settings.search.editTitle': 'Змінити пошукову систему',
  'settings.search.addLead':
    'Знайдіть щось на сайті, скопіюйте адресу сторінки з результатами й поставте %s на місце свого запиту, наприклад https://uk.wikipedia.org/w/index.php?search=%s.',
  'settings.search.field.name': 'Назва',
  'settings.search.field.namePlaceholder': 'Вікіпедія',
  'settings.search.field.url': 'Адреса пошуку',
  'settings.search.field.urlHelp': 'Починається з https:// і містить %s там, де має бути запит.',
  'settings.search.field.keyword': 'Ключове слово',
  'settings.search.field.keywordHelp': 'Необов’язково, без пробілів. Якщо порожньо — утвориться з назви.',
  'settings.search.addButton': 'Додати систему',
  'settings.search.added': '{name} додано',
  'settings.search.updated': '{name} збережено',

  'settings.search.err.name.required': 'Дайте системі назву.',
  'settings.search.err.name.tooLong': 'Не більше 60 символів.',
  'settings.search.err.url.required': 'Введіть адресу пошуку.',
  'settings.search.err.url.scheme': 'Адреса має починатися з https:// або http://.',
  'settings.search.err.url.spaces': 'В адресі не може бути пробілів.',
  'settings.search.err.url.placeholder': 'Поставте %s там, де має бути запит.',
  'settings.search.err.keyword.spaces': 'У ключовому слові не може бути пробілів чи %.',
  'settings.search.err.keyword.tooLong': 'Не більше 20 символів.',
  'settings.search.err.keyword.taken': 'Це ключове слово вже використовує інша система.',

  'settings.search.tryTitle': 'Перевірити',
  'settings.search.tryLabel': 'Введіть, як в адресному рядку',
  'settings.search.tryHelp': 'Показує, що відкриється після Enter з налаштуваннями вище.',
  'settings.search.tryPlaceholder': 'погода Київ',
  'settings.search.trySearch': 'Пошук ({engine}):',
  'settings.search.tryOpen': 'Відкриється адреса:',

  'settings.search.omniboxSearch': 'Шукати ({engine})',
  'settings.search.omniboxOpen': 'Відкрити',
};
