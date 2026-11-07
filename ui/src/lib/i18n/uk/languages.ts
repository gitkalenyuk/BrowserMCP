// Налаштування → Загальні → Мова: мови для сайтів, мова браузера, перевірка правопису
// (settings/WebLanguages.svelte).
export default {
  'languages.web': 'Мови для сайтів',
  'languages.webHelp':
    'Сайти бачать ці мови в порядку пріоритету (заголовок Accept-Language і navigator.languages) і за ними вибирають, якою мовою показати сторінку. Зміни одразу діють у всіх вкладках; перезавантажте сторінку, щоб побачити її новою мовою.',
  'languages.automatic': 'автоматично',
  'languages.automaticHint': 'Мова інтерфейсу, потім англійська',
  'languages.useAutomatic': 'Автоматично',
  'languages.automaticBack': 'Сайти отримують мову інтерфейсу, потім англійську',
  'languages.loadError': 'Не вдалося прочитати мовні налаштування: {error}',
  'languages.saved': 'Мови для сайтів збережено',
  'languages.spell': 'Правопис',
  'languages.spellHint': 'Перевіряти правопис цією мовою в текстових полях',
  'languages.spellFor': 'Перевірка правопису: {name}',
  'languages.spellSaved': 'Мови перевірки правопису збережено',
  'languages.spellUnsupported': 'У цій збірці браузера перевірку правопису налаштувати не можна.',
  'languages.moveUp': 'Перемістити «{name}» вище',
  'languages.moveDown': 'Перемістити «{name}» нижче',
  'languages.remove': 'Прибрати «{name}»',
  'languages.keepOne': 'Для сайтів потрібна хоча б одна мова',
  'languages.addPlaceholder': 'Додати мову: Deutsch, fr, pt-BR…',
  'languages.addLabel': 'Мова, яку додати',
  'languages.err.unknown': '«{text}» — не мова: виберіть зі списку або введіть код на кшталт de чи pt-BR',
  'languages.err.listed': '{name} уже є в списку',
  'languages.precedence':
    'Профілі зі стелс-ідентичністю зберігають власні мови, підібрані під країну свого проксі: сайт має бачити мову, що відповідає IP-адресі.',
  'languages.ownProfiles': 'Зараз зберігають власні:',
  'languages.browser': 'Мова браузера',
  'languages.browserHelp':
    'Мова самого Chromium: контекстне меню сторінки, його діалоги й сторінки помилок. У списку лише мови, що постачаються з цим браузером.',
  'languages.noLocales': 'Поруч із браузером не знайдено мовних файлів Chromium (теки locales).',
  'languages.system': 'Як у Windows',
  'languages.restart': 'Перезапустіть браузер, щоб застосувати',
  'languages.browserSaved': 'Мову браузера збережено: перезапустіть браузер, щоб її застосувати',
};
