import { afterEach, describe, expect, it } from 'vitest';
import { COMMANDS } from '../../../../sdk/protocol/src/generated/commands';
import { DICTS } from '../../lib/i18n/index.svelte';
import { COMMAND_IDS, DEFAULT_KEYMAP, parseChord } from '../../lib/keymap';
import { SETTINGS_SECTIONS, isSettingsUrl, openGuide, parseGuideHash, peekGuideAnchor, takeSettingsRequest } from '../../lib/settings/open';
import { HELP_FOR, parseAnchor, resolveAnchor } from './anchors';
import { GUIDE } from './content';
import { commandKey, highlight, parseInline, plainText, queryWords } from './inline';
import { CHAPTER_IDS, type Block, type Chapter } from './model';
import { searchGuide, snippetAround } from './search';
import { argsJson, snippet, wireName } from './snippets';

const LOCALES = ['en', 'uk'] as const;
const allBlocks = (chapters: Chapter[]) => chapters.flatMap((c) => c.sections.flatMap((s) => s.blocks.map((b) => ({ c, s, b }))));

/** What must be identical in every language: the "shape" of a block, without its prose. */
function shape(b: Block): unknown {
  switch (b.type) {
    case 'steps':
    case 'list':
      return [b.type, b.items.length];
    case 'terms':
      return [b.type, b.items.length];
    case 'keys':
      return [b.type, b.items.map((k) => ('keys' in k ? k.keys : k.command))];
    case 'settings':
      return [b.type, b.section, !!b.text];
    case 'api':
      // The same fields, but the values may be localised (a prompt, a question, a search).
      return [b.type, b.command, argShape(b.args), !!b.text];
    case 'see':
      return [b.type, b.chapter, b.section ?? null, !!b.text];
    case 'code':
      return [b.type, b.lang];
    default:
      return [b.type];
  }
}

/** The structure of example arguments: keys and value types, not the values. */
function argShape(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(argShape);
  if (v && typeof v === 'object') return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, argShape(x)]));
  return typeof v;
}

/** Every prose string of a block (inline marks allowed). */
function prose(b: Block): string[] {
  switch (b.type) {
    case 'p':
    case 'tip':
    case 'warning':
      return [b.text];
    case 'steps':
    case 'list':
      return b.items;
    case 'keys':
      return b.items.map((k) => k.text);
    case 'terms':
      return b.items.flatMap((i) => [i.term, i.text]);
    case 'settings':
    case 'see':
    case 'api':
      return b.text ? [b.text] : [];
    case 'code':
      return [];
  }
}

describe('guide content', () => {
  it('every language has the same chapters, in order', () => {
    for (const loc of LOCALES) expect(GUIDE[loc].map((c) => c.id), loc).toEqual([...CHAPTER_IDS]);
  });

  it('Ukrainian covers exactly what English covers (sections, blocks, commands, links)', () => {
    GUIDE.en.forEach((en, i) => {
      const uk = GUIDE.uk[i];
      expect(uk.sections.map((s) => s.id), en.id).toEqual(en.sections.map((s) => s.id));
      en.sections.forEach((s, j) => {
        expect(uk.sections[j].blocks.map(shape), `${en.id}/${s.id}`).toEqual(s.blocks.map(shape));
      });
    });
  });

  it('ids are unique and fit #guide/<chapter>/<section>', () => {
    for (const loc of LOCALES) {
      for (const c of GUIDE[loc]) {
        const ids = c.sections.map((s) => s.id);
        expect(new Set(ids).size, `${loc} ${c.id}`).toBe(ids.length);
        for (const id of ids) expect(parseGuideHash(`#guide/${c.id}/${id}`), id).toBe(`${c.id}/${id}`);
      }
    }
  });

  it('has no empty text and no unbalanced inline marks', () => {
    for (const loc of LOCALES) {
      for (const c of GUIDE[loc]) {
        for (const text of [c.title, c.summary, ...c.sections.map((s) => s.title)]) expect(text.trim(), `${loc} ${c.id}`).not.toBe('');
      }
      for (const { c, s, b } of allBlocks(GUIDE[loc])) {
        for (const text of prose(b)) {
          const where = `${loc} ${c.id}/${s.id}: ${text.slice(0, 60)}`;
          expect(text.trim(), where).not.toBe('');
          const rest = parseInline(text)
            .filter((sp) => sp.mark === 'text')
            .map((sp) => sp.text)
            .join('');
          expect(rest, where).not.toMatch(/`|\*\*|\[\[|\]\]/);
        }
      }
    }
  });

  it('every chapter but the reference ones is honest about its limits', () => {
    for (const loc of LOCALES) {
      for (const c of GUIDE[loc].filter((x) => !['troubleshooting', 'glossary'].includes(x.id))) {
        expect(c.sections.some((s) => s.id === 'limits'), `${loc} ${c.id}`).toBe(true);
      }
    }
  });

  it('settings links point at real sections that have a name in both languages', () => {
    for (const loc of LOCALES) {
      for (const { c, b } of allBlocks(GUIDE[loc])) {
        if (b.type !== 'settings') continue;
        const where = `${loc} ${c.id} → ${b.section}`;
        expect((SETTINGS_SECTIONS as readonly string[]).includes(b.section), where).toBe(true);
        expect(b.section, where).not.toBe('guide');
        for (const l of LOCALES) expect(DICTS[l][`settings.nav.${b.section}`], `${where} (${l})`).toBeTruthy();
      }
    }
    // The guide names sections only by their nav labels: no labels of its own are left.
    for (const loc of LOCALES) expect(Object.keys(DICTS[loc]).filter((k) => k.startsWith('guide.pendingSection.')), loc).toEqual([]);
  });

  it('every settings section has a nav label in both languages', () => {
    for (const id of SETTINGS_SECTIONS) for (const loc of LOCALES) expect(DICTS[loc][`settings.nav.${id}`], `${loc} ${id}`).toBeTruthy();
  });

  it('shortcuts are keymap commands or keys the parser understands', () => {
    for (const { c, b } of allBlocks(GUIDE.en)) {
      if (b.type !== 'keys') continue;
      for (const item of b.items) {
        if ('command' in item) expect((COMMAND_IDS as readonly string[]).includes(item.command), `${c.id}: ${item.command}`).toBe(true);
        else for (const k of item.keys) expect(parseChord(k), `${c.id}: ${k}`).not.toBeNull();
      }
    }
  });

  it('shortcuts written in the text are keymap commands or keys the parser understands', () => {
    for (const loc of LOCALES) {
      for (const { c, s, b } of allBlocks(GUIDE[loc])) {
        for (const span of prose(b).flatMap(parseInline)) {
          const where = `${loc} ${c.id}/${s.id}: [[${span.text}]]`;
          if (span.mark === 'cmd') expect((COMMAND_IDS as readonly string[]).includes(span.text), where).toBe(true);
          if (span.mark === 'kbd') expect(parseChord(span.text), where).not.toBeNull();
        }
      }
    }
  });

  it('both languages show the same shortcuts in the same blocks', () => {
    const keysOf = (b: Block) =>
      prose(b)
        .flatMap(parseInline)
        .filter((sp) => sp.mark === 'kbd' || sp.mark === 'cmd')
        .map((sp) => `${sp.mark}:${sp.text}`)
        .sort();
    GUIDE.en.forEach((en, i) =>
      en.sections.forEach((s, j) =>
        s.blocks.forEach((b, k) => expect(keysOf(GUIDE.uk[i].sections[j].blocks[k]), `${en.id}/${s.id} #${k}`).toEqual(keysOf(b))),
      ),
    );
  });

  it('API examples name real commands with their MCP tool names', () => {
    const blocks = allBlocks(GUIDE.en).filter(({ b }) => b.type === 'api');
    expect(blocks.length).toBeGreaterThan(40);
    for (const { c, b } of blocks) {
      if (b.type !== 'api') continue;
      const meta = (COMMANDS as Record<string, { wireName: string; toolset: string }>)[b.command];
      expect(meta, `${c.id}: ${b.command}`).toBeDefined();
      expect(wireName(b.command)).toBe(meta.wireName);
      expect(meta.toolset, `${b.command} is internal`).not.toBe('hidden');
    }
  });

  it('"see also" links and Help links resolve', () => {
    for (const loc of LOCALES) {
      for (const { c, b } of allBlocks(GUIDE[loc])) {
        if (b.type !== 'see') continue;
        expect(resolveAnchor(GUIDE[loc], { chapter: b.chapter, section: b.section }), `${loc} ${c.id} → ${b.chapter}/${b.section}`).toEqual(
          b.section ? { chapter: b.chapter, section: b.section } : { chapter: b.chapter },
        );
      }
      for (const [id, anchor] of Object.entries(HELP_FOR)) expect(resolveAnchor(GUIDE[loc], anchor), `${loc} help ${id}`).toEqual(anchor);
    }
    for (const id of SETTINGS_SECTIONS.filter((s) => s !== 'guide')) expect((HELP_FOR as Record<string, unknown>)[id], id).toBeDefined();
  });

  it('has no paragraphs left that wait for features to be merged', () => {
    // Paragraphs written before their feature existed were marked `// VERIFY-AFTER-MERGE`;
    // every one has been checked against the merged code since.
    const files = import.meta.glob(['./*.ts', './content/*/*.ts'], { query: '?raw', import: 'default', eager: true }) as Record<string, string>;
    expect(Object.keys(files).length).toBeGreaterThan(10);
    const marked = Object.entries(files)
      .filter(([path, text]) => !path.endsWith('.test.ts') && text.includes('VERIFY-AFTER-MERGE'))
      .map(([path]) => path);
    expect(marked).toEqual([]);
  });
});

describe('inline marks', () => {
  it('parses code, bold and keys', () => {
    expect(parseInline('Press [[Ctrl+K]], type `tabs` and **Enter**.')).toEqual([
      { mark: 'text', text: 'Press ' },
      { mark: 'kbd', text: 'Ctrl+K' },
      { mark: 'text', text: ', type ' },
      { mark: 'code', text: 'tabs' },
      { mark: 'text', text: ' and ' },
      { mark: 'strong', text: 'Enter' },
      { mark: 'text', text: '.' },
    ]);
    expect(parseInline('`a **b**`')).toEqual([{ mark: 'code', text: 'a **b**' }]);
    expect(plainText('**Settings → AI**')).toBe('Settings → AI');
  });

  it('[[cmd:<id>]] names a keymap command and shows its current key', () => {
    expect(parseInline('AI panel ([[cmd:panel.ai]])')).toEqual([
      { mark: 'text', text: 'AI panel (' },
      { mark: 'cmd', text: 'panel.ai' },
      { mark: 'text', text: ')' },
    ]);
    expect(commandKey('panel.ai')).toBe('Ctrl+Shift+3');
    expect(commandKey('panel.ai', { ...DEFAULT_KEYMAP, 'panel.ai': ['Alt+A', 'Alt+B'] })).toBe('Alt+A');
    expect(commandKey('panel.ai', { ...DEFAULT_KEYMAP, 'panel.ai': [] })).toBeNull();
    expect(commandKey('no.such')).toBeNull();
    // Search sees the default key, like the rest of the plain text.
    expect(plainText('Open it with [[cmd:panel.ai]].')).toBe('Open it with Ctrl+Shift+3.');
  });

  it('highlights every case-insensitive match and merges overlaps', () => {
    expect(highlight('Tabs and tab groups', ['tab', 'tabs'])).toEqual([
      { text: 'Tabs', hit: true },
      { text: ' and ', hit: false },
      { text: 'tab', hit: true },
      { text: ' groups', hit: false },
    ]);
    expect(highlight('Проксі й пули', ['проксі'])).toEqual([
      { text: 'Проксі', hit: true },
      { text: ' й пули', hit: false },
    ]);
    expect(highlight('abc', [])).toEqual([{ text: 'abc', hit: false }]);
    expect(queryWords('  Proxy  pool proxy ')).toEqual(['proxy', 'pool']);
  });
});

describe('search', () => {
  it('finds chapters that contain every word, best first, in each language', () => {
    const en = searchGuide(GUIDE.en, queryWords('ffmpeg'));
    expect(en.map((h) => h.chapter)).toContain('recording');
    expect(en.map((h) => h.chapter)).toContain('troubleshooting');
    const uk = searchGuide(GUIDE.uk, queryWords('проксі пул'));
    expect(uk[0]?.chapter).toBe('stealth');
    expect(searchGuide(GUIDE.en, queryWords('ffmpeg zzzz-nothing'))).toEqual([]);
    expect(searchGuide(GUIDE.en, [])).toEqual([]);
  });

  it('finds commands by their dotted and MCP names', () => {
    expect(searchGuide(GUIDE.en, ['net_rule_add']).map((h) => h.chapter)).toContain('traffic');
    expect(searchGuide(GUIDE.uk, ['page.snapshot']).map((h) => h.chapter)).toContain('perception');
  });

  it('cuts snippets around the first match', () => {
    const long = `${'a'.repeat(200)} needle ${'b'.repeat(200)}`;
    const s = snippetAround(long, 'needle');
    expect(s).toContain('needle');
    expect(s.startsWith('…') && s.endsWith('…')).toBe(true);
    expect(snippetAround('short text', 'text')).toBe('short text');
  });
});

describe('API snippets', () => {
  it('formats MCP, PowerShell, curl and SDK calls', () => {
    const args = { url: "https://example.com/?q=it's" };
    expect(snippet('mcp', 'nav.goto', args)).toBe(`nav_goto { "url": "https://example.com/?q=it's" }`);
    expect(snippet('powershell', 'nav.goto', args)).toBe(
      `Invoke-RestMethod -Method Post http://127.0.0.1:47800/v1/cmd/nav.goto -ContentType 'application/json; charset=utf-8' -Body '{"url":"https://example.com/?q=it''s"}'`,
    );
    expect(snippet('curl', 'nav.goto', args)).toBe(
      `curl -X POST http://127.0.0.1:47800/v1/cmd/nav.goto -H "content-type: application/json" -d '{"url":"https://example.com/?q=it'\\''s"}'`,
    );
    expect(snippet('sdk', 'net.rule_add', { action: 'block' })).toBe('await browser.net.rule_add({ "action": "block" });');
    expect(snippet('sdk', 'tabs.list', {})).toBe('await browser.tabs.list({});');
  });

  it('keeps short arguments on one line and indents long ones', () => {
    expect(argsJson({})).toBe('{}');
    const long = argsJson({ script: 'login', data: { csv_path: 'C:\\data\\logins.csv' }, parallel: 4, retries: 1 });
    expect(long.split('\n').length).toBeGreaterThan(3);
    expect(JSON.parse(long)).toEqual({ script: 'login', data: { csv_path: 'C:\\data\\logins.csv' }, parallel: 4, retries: 1 });
  });
});

describe('deep links', () => {
  const store = new Map<string, string>();
  const fakeWindow = {
    localStorage: {
      getItem: (k: string) => store.get(k) ?? null,
      setItem: (k: string, v: string) => void store.set(k, v),
    },
  };
  afterEach(() => {
    store.clear();
    delete (globalThis as { window?: unknown }).window;
  });

  it('parses #guide/<chapter>[/<section>] and nothing else', () => {
    expect(parseGuideHash('#guide/tabs')).toBe('tabs');
    expect(parseGuideHash('#guide/tabs/split')).toBe('tabs/split');
    expect(parseGuideHash('guide/ai-panel/modes')).toBe('ai-panel/modes');
    for (const bad of ['', '#', '#guide/', '#guide/Tabs', '#guide/a/b/c', '#guide/<x>', '#other/tabs', '#guide/tabs?x=1']) {
      expect(parseGuideHash(bad), bad).toBeNull();
    }
    expect(parseAnchor('tabs/split')).toEqual({ chapter: 'tabs', section: 'split' });
    expect(parseAnchor(null)).toBeNull();
  });

  it('falls back to the chapter when a section is unknown', () => {
    expect(resolveAnchor(GUIDE.en, { chapter: 'tabs', section: 'nope' })).toEqual({ chapter: 'tabs' });
    expect(resolveAnchor(GUIDE.en, { chapter: 'nope' })).toBeNull();
  });

  it('a settings tab with an anchor still counts as the settings tab', () => {
    expect(isSettingsUrl('bmcp://ui/settings.html')).toBe(true);
    expect(isSettingsUrl('bmcp://ui/settings.html#guide/tabs')).toBe(true);
    expect(isSettingsUrl('bmcp://ui/settings.html?x=1')).toBe(false);
  });

  it('openGuide asks the settings page for the guide at an anchor', () => {
    (globalThis as { window?: unknown }).window = fakeWindow;
    const opened: string[] = [];
    const chrome = { tabs: { tabs: [{ id: 7, url: 'https://example.com' }] }, activate: () => opened.push('activate'), newTab: (u: string) => opened.push(u) };
    const now = Date.now();
    openGuide(chrome, 'traffic/limits', now);
    expect(opened).toEqual(['bmcp://ui/settings.html']);
    expect(peekGuideAnchor(now)).toBe('traffic/limits');
    expect(takeSettingsRequest(now)).toBe('guide');
    expect(peekGuideAnchor(now)).toBeNull();
  });
});
