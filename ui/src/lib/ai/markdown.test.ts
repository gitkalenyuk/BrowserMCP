import { describe, expect, it } from 'vitest';
import { inlineText, parseInline, parseMarkdown, safeHref, type Block, type Inline } from './markdown';

const text = (v: string): Inline => ({ t: 'text', v });

/** Every node of a tree, to look for links and tags anywhere. */
function walk(blocks: Block[]): (Block | Inline)[] {
  const out: (Block | Inline)[] = [];
  const inl = (list: Inline[]) =>
    list.forEach((n) => {
      out.push(n);
      if ('c' in n) inl(n.c);
    });
  const blk = (list: Block[]) =>
    list.forEach((b) => {
      out.push(b);
      if (b.t === 'h' || b.t === 'p') inl(b.c);
      if (b.t === 'quote') blk(b.c);
      if (b.t === 'list') b.items.forEach(blk);
      if (b.t === 'table') [b.head, ...b.rows].flat().forEach(inl);
    });
  blk(blocks);
  return out;
}

describe('markdown blocks', () => {
  it('headings, paragraphs with kept line breaks, rules', () => {
    expect(parseMarkdown('# Title\n\nline one\nline two\n\n---\n### Small')).toEqual([
      { t: 'h', level: 1, c: [text('Title')] },
      { t: 'p', c: [text('line one'), { t: 'br' }, text('line two')] },
      { t: 'hr' },
      { t: 'h', level: 3, c: [text('Small')] },
    ]);
  });

  it('fenced code keeps its text verbatim, with the language; an unclosed fence is still code', () => {
    expect(parseMarkdown('```ts\nconst a = 1 < 2;\n**not bold**\n```\nafter')).toEqual([
      { t: 'code', lang: 'ts', text: 'const a = 1 < 2;\n**not bold**' },
      { t: 'p', c: [text('after')] },
    ]);
    expect(parseMarkdown('```py\nprint(1)')).toEqual([{ t: 'code', lang: 'py', text: 'print(1)' }]);
  });

  it('bullet and numbered lists, nested', () => {
    const [list] = parseMarkdown('1. one\n2. two\n   - a\n   - b\n3. three');
    expect(list).toMatchObject({ t: 'list', ordered: true, start: 1 });
    if (list.t !== 'list') throw new Error();
    expect(list.items).toHaveLength(3);
    expect(list.items[1][1]).toMatchObject({ t: 'list', ordered: false });
    expect(parseMarkdown('- x\n- y\n\npara')).toEqual([
      { t: 'list', ordered: false, start: 1, items: [[{ t: 'p', c: [text('x')] }], [{ t: 'p', c: [text('y')] }]] },
      { t: 'p', c: [text('para')] },
    ]);
  });

  it('a paragraph ends where a list starts', () => {
    expect(parseMarkdown('Steps:\n- a\n- b').map((b) => b.t)).toEqual(['p', 'list']);
  });

  it('GFM tables with alignment', () => {
    const [table] = parseMarkdown('| Name | Price |\n|:-----|------:|\n| X1 | $1,499 |\n| T14s | `1129` |');
    expect(table).toEqual({
      t: 'table',
      align: ['left', 'right'],
      head: [[text('Name')], [text('Price')]],
      rows: [
        [[text('X1')], [text('$1,499')]],
        [[text('T14s')], [{ t: 'code', v: '1129' }]],
      ],
    });
  });

  it('block quotes', () => {
    expect(parseMarkdown('> quoted\n> more')).toEqual([{ t: 'quote', c: [{ t: 'p', c: [text('quoted'), { t: 'br' }, text('more')] }] }]);
  });
});

describe('markdown inline', () => {
  it('bold, italics, strikethrough, inline code', () => {
    expect(parseInline('a **b** *c* _d_ ~~e~~ `f*g*`')).toEqual([
      text('a '),
      { t: 'strong', c: [text('b')] },
      text(' '),
      { t: 'em', c: [text('c')] },
      text(' '),
      { t: 'em', c: [text('d')] },
      text(' '),
      { t: 'del', c: [text('e')] },
      text(' '),
      { t: 'code', v: 'f*g*' },
    ]);
  });

  it('leaves snake_case words and lone stars alone', () => {
    expect(parseInline('use page_read_all and 2 * 3 * 4')).toEqual([text('use page_read_all and 2 * 3 * 4')]);
  });

  it('links, autolinks and bare URLs', () => {
    expect(parseInline('[docs](https://example.com/a "t") <https://x.io> see https://a.b/c.')).toEqual([
      { t: 'link', href: 'https://example.com/a', c: [text('docs')] },
      text(' '),
      { t: 'link', href: 'https://x.io', c: [text('https://x.io')] },
      text(' see '),
      { t: 'link', href: 'https://a.b/c', c: [text('https://a.b/c')] },
      text('.'),
    ]);
  });

  it('backslash escapes', () => {
    expect(parseInline('\\*not em\\*')).toEqual([text('*not em*')]);
  });
});

describe('markdown sanitising', () => {
  it('raw HTML stays text: no element, no handler', () => {
    const tree = parseMarkdown('<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>');
    expect(tree).toEqual([
      { t: 'p', c: [text('<script>alert(1)</script>')] },
      { t: 'p', c: [text('<img src=x onerror=alert(1)>')] },
    ]);
  });

  it('drops dangerous link targets but keeps their text', () => {
    const nodes = walk(parseMarkdown('[click](javascript:alert(1)) [d](data:text/html,x) [v](vbscript:x) <javascript:alert(1)> [f](file:///C:/x) [ui](bmcp://ui/settings.html)'));
    expect(nodes.filter((n) => n.t === 'link')).toEqual([]);
    expect(inlineText((parseMarkdown('[click](javascript:alert(1))')[0] as { c: Inline[] }).c)).toBe('click');
  });

  it('images become plain links, never loaded', () => {
    expect(parseInline('![chart](https://e.com/c.png)')).toEqual([{ t: 'link', href: 'https://e.com/c.png', c: [text('chart')] }]);
    expect(parseInline('![x](javascript:1)')).toEqual([text('x')]);
  });

  it('safeHref keeps web and mail addresses only', () => {
    expect(safeHref('https://e.com')).toBe('https://e.com');
    expect(safeHref('mailto:a@b.c')).toBe('mailto:a@b.c');
    expect(safeHref(' JavaScript:alert(1)')).toBeNull();
    expect(safeHref('/relative')).toBeNull();
  });
});
