// A small Markdown parser for the AI panel's answers: text → a tree of blocks and inline
// marks, rendered by AiMarkdown.svelte with ordinary Svelte elements. Nothing is ever turned
// into HTML from a string ({@html} is not used), so whatever the model writes, raw HTML
// included, is shown as text: `<script>` cannot run in the privileged bmcp://ui origin.
// Links keep only http(s) and mailto addresses; images become links (no remote loads).
//
// Covers what models write: headings, paragraphs (single newlines kept as line breaks),
// bullet and numbered lists (nested), block quotes, rules, GFM tables, fenced code blocks
// (an unclosed fence while streaming is still a code block), inline code, bold, italics,
// strikethrough, links, <autolinks> and bare URLs. Unit-tested in markdown.test.ts.

import { isSafeLinkUrl } from '../safeurl';

export type Inline =
  | { t: 'text'; v: string }
  | { t: 'code'; v: string }
  | { t: 'strong'; c: Inline[] }
  | { t: 'em'; c: Inline[] }
  | { t: 'del'; c: Inline[] }
  | { t: 'link'; href: string; c: Inline[] }
  | { t: 'br' };

export type Align = 'left' | 'center' | 'right' | null;

export type Block =
  | { t: 'h'; level: 1 | 2 | 3 | 4 | 5 | 6; c: Inline[] }
  | { t: 'p'; c: Inline[] }
  | { t: 'code'; lang: string; text: string }
  | { t: 'list'; ordered: boolean; start: number; items: Block[][] }
  | { t: 'quote'; c: Block[] }
  | { t: 'hr' }
  | { t: 'table'; align: Align[]; head: Inline[][]; rows: Inline[][][] };

/** A link target the panel may open: http(s) or mailto; anything else is dropped. */
export function safeHref(url: string): string | null {
  const u = url.trim();
  if (isSafeLinkUrl(u)) return u;
  if (/^mailto:[^\s<>"]+@[^\s<>"]+$/i.test(u)) return u;
  return null;
}

// ---- blocks ---------------------------------------------------------------------------

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([^\s`]*)[^`]*$/;
const HEADING = /^ {0,3}(#{1,6})(?:\s+(.*?))?\s*#*\s*$/;
const RULE = /^ {0,3}([-*_])(?:\s*\1){2,}\s*$/;
const QUOTE = /^ {0,3}> ?(.*)$/;
const ITEM = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const TABLE_SEP = /^\s*\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$/;

const indentOf = (line: string) => line.replace(/\t/g, '    ').match(/^ */)![0].length;
const blank = (line: string) => line.trim() === '';

function splitRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith('|')) s = s.slice(1);
  if (s.endsWith('|') && !s.endsWith('\\|')) s = s.slice(0, -1);
  const cells: string[] = [];
  let cur = '';
  let code = false;
  for (let i = 0; i < s.length; i++) {
    const ch = s[i];
    if (ch === '\\' && s[i + 1] === '|') {
      cur += '|';
      i++;
    } else if (ch === '`') {
      code = !code;
      cur += ch;
    } else if (ch === '|' && !code) {
      cells.push(cur.trim());
      cur = '';
    } else cur += ch;
  }
  cells.push(cur.trim());
  return cells;
}

function isTableStart(lines: string[], i: number): boolean {
  return lines[i].includes('|') && i + 1 < lines.length && TABLE_SEP.test(lines[i + 1]) && lines[i + 1].includes('-');
}

/** Lines that end a paragraph (another block starts). */
function startsBlock(lines: string[], i: number): boolean {
  const line = lines[i];
  return (
    FENCE.test(line) ||
    HEADING.test(line) ||
    RULE.test(line) ||
    QUOTE.test(line) ||
    ITEM.test(line) ||
    isTableStart(lines, i)
  );
}

export function parseMarkdown(text: string): Block[] {
  return parseBlocks(text.replace(/\r\n?/g, '\n').split('\n'));
}

function parseBlocks(lines: string[]): Block[] {
  const out: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (blank(line)) {
      i++;
      continue;
    }
    const fence = FENCE.exec(line);
    if (fence) {
      const mark = fence[1];
      const body: string[] = [];
      i++;
      while (i < lines.length && !new RegExp(`^ {0,3}${mark[0]}{${mark.length},}\\s*$`).test(lines[i])) body.push(lines[i++]);
      i++; // the closing fence (or the end of an unfinished answer)
      out.push({ t: 'code', lang: fence[2].toLowerCase(), text: body.join('\n') });
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      out.push({ t: 'h', level: heading[1].length as 1, c: parseInline(heading[2] ?? '') });
      i++;
      continue;
    }
    if (RULE.test(line)) {
      out.push({ t: 'hr' });
      i++;
      continue;
    }
    if (QUOTE.test(line)) {
      const inner: string[] = [];
      while (i < lines.length && !blank(lines[i])) {
        const q = QUOTE.exec(lines[i]);
        if (q) inner.push(q[1]);
        else if (startsBlock(lines, i)) break;
        else inner.push(lines[i]);
        i++;
      }
      out.push({ t: 'quote', c: parseBlocks(inner) });
      continue;
    }
    if (ITEM.test(line)) {
      i = parseList(lines, i, out);
      continue;
    }
    if (isTableStart(lines, i)) {
      const head = splitRow(line);
      const align = splitRow(lines[i + 1]).map((c): Align => {
        const l = c.startsWith(':');
        const r = c.endsWith(':');
        return l && r ? 'center' : r ? 'right' : l ? 'left' : null;
      });
      i += 2;
      const rows: Inline[][][] = [];
      while (i < lines.length && !blank(lines[i]) && lines[i].includes('|')) {
        const cells = splitRow(lines[i++]);
        rows.push(head.map((_, k) => parseInline(cells[k] ?? '')));
      }
      out.push({ t: 'table', align: head.map((_, k) => align[k] ?? null), head: head.map((h) => parseInline(h)), rows });
      continue;
    }
    const para: string[] = [line.trim()];
    i++;
    while (i < lines.length && !blank(lines[i]) && !startsBlock(lines, i)) para.push(lines[i++].trim());
    out.push({ t: 'p', c: parseInline(para.join('\n')) });
  }
  return out;
}

/** One list from line `i` (bullets or numbers, whichever it starts with); returns the next line. */
function parseList(lines: string[], i: number, out: Block[]): number {
  const first = ITEM.exec(lines[i])!;
  const base = indentOf(first[1]);
  const ordered = /\d/.test(first[2]);
  const start = ordered ? parseInt(first[2], 10) : 1;
  const items: string[][] = [];
  let content = 0;
  let gap = false;
  while (i < lines.length) {
    const line = lines[i];
    const m = ITEM.exec(line);
    const indent = indentOf(line);
    if (m && indent <= base + 1 && /\d/.test(m[2]) === ordered) {
      items.push([m[3]]);
      content = indent + m[2].length + 1;
      gap = false;
      i++;
      continue;
    }
    if (m && indent <= base + 1) break; // the other kind of list starts
    if (blank(line)) {
      gap = true;
      i++;
      continue;
    }
    const cur = items[items.length - 1];
    if (indent > base) {
      // Nested content (a sub-list, a code block…) or a continuation line.
      if (gap) cur.push('');
      cur.push(line.replace(/\t/g, '    ').slice(Math.min(indent, content)));
      gap = false;
      i++;
      continue;
    }
    if (!gap && !startsBlock(lines, i)) {
      cur.push(line.trim()); // lazy continuation of the item's paragraph
      i++;
      continue;
    }
    break;
  }
  out.push({ t: 'list', ordered, start, items: items.map((l) => parseBlocks(l)) });
  return i;
}

// ---- inline -----------------------------------------------------------------------------

const PUNCT = /[!"#$%&'()*+,\-./:;<=>?@[\\\]^_`{|}~]/;
const BARE_URL = /^(?:https?:\/\/|www\.)[^\s<>"'`]+/i;

function pushText(out: Inline[], v: string): void {
  if (!v) return;
  const last = out[out.length - 1];
  if (last?.t === 'text') last.v += v;
  else out.push({ t: 'text', v });
}

/** The end of `[label]` starting at `open` (brackets may nest), or -1. */
function closeBracket(s: string, open: number): number {
  let depth = 0;
  for (let i = open; i < s.length; i++) {
    if (s[i] === '\\') i++;
    else if (s[i] === '[') depth++;
    else if (s[i] === ']' && --depth === 0) return i;
  }
  return -1;
}

/** The end of `(url "title")` starting at `open`, or -1. */
function closeParen(s: string, open: number): number {
  let depth = 0;
  for (let i = open; i < s.length; i++) {
    if (s[i] === '\\') i++;
    else if (s[i] === '(') depth++;
    else if (s[i] === ')' && --depth === 0) return i;
    else if (s[i] === '\n') return -1;
  }
  return -1;
}

/** Position of the closing delimiter `d` for an opening at `from`, or -1. */
function findClose(s: string, from: number, d: string): number {
  for (let i = from; i < s.length; i++) {
    if (s[i] === '\\') {
      i++;
      continue;
    }
    if (s[i] === '`') {
      const run = s.slice(i).match(/^`+/)![0];
      const end = s.indexOf(run, i + run.length);
      if (end > 0) i = end + run.length - 1;
      continue;
    }
    if (s.startsWith(d, i) && i > from && !/\s/.test(s[i - 1])) {
      // `*` closes only a single star, not the start of `**`.
      if (d.length === 1 && (s[i + 1] === d || s[i - 1] === d)) continue;
      if (d === '_' && /\w/.test(s[i + 1] ?? '')) continue;
      return i;
    }
  }
  return -1;
}

export function parseInline(s: string): Inline[] {
  const out: Inline[] = [];
  let i = 0;
  while (i < s.length) {
    const ch = s[i];
    if (ch === '\\' && i + 1 < s.length && PUNCT.test(s[i + 1])) {
      pushText(out, s[i + 1]);
      i += 2;
      continue;
    }
    if (ch === '\\' && s[i + 1] === '\n') {
      out.push({ t: 'br' });
      i += 2;
      continue;
    }
    if (ch === '\n') {
      out.push({ t: 'br' });
      i++;
      continue;
    }
    if (ch === '`') {
      const run = s.slice(i).match(/^`+/)![0];
      const end = s.indexOf(run, i + run.length);
      if (end > 0) {
        let v = s.slice(i + run.length, end).replace(/\n/g, ' ');
        if (v.startsWith(' ') && v.endsWith(' ') && v.trim()) v = v.slice(1, -1);
        out.push({ t: 'code', v });
        i = end + run.length;
        continue;
      }
      pushText(out, run);
      i += run.length;
      continue;
    }
    // A few HTML line breaks models put into table cells; any other tag stays text.
    const br = /^<br\s*\/?>/i.exec(s.slice(i));
    if (br) {
      out.push({ t: 'br' });
      i += br[0].length;
      continue;
    }
    if (ch === '<') {
      const auto = /^<((?:https?:\/\/|mailto:)[^\s<>]+)>/i.exec(s.slice(i));
      const href = auto && safeHref(auto[1]);
      if (auto && href) {
        out.push({ t: 'link', href, c: [{ t: 'text', v: auto[1].replace(/^mailto:/i, '') }] });
        i += auto[0].length;
        continue;
      }
    }
    const image = ch === '!' && s[i + 1] === '[';
    if (ch === '[' || image) {
      const open = image ? i + 1 : i;
      const close = closeBracket(s, open);
      if (close > 0 && s[close + 1] === '(') {
        const end = closeParen(s, close + 1);
        if (end > 0) {
          const target = s.slice(close + 2, end).trim();
          const url = target.replace(/\s+(["'(]).*$/, '').replace(/^<|>$/g, '');
          const label = s.slice(open + 1, close);
          const href = safeHref(url);
          const c = parseInline(label || url);
          if (href) out.push({ t: 'link', href, c: c.length ? c : [{ t: 'text', v: url }] });
          else out.push(...c);
          i = end + 1;
          continue;
        }
      }
    }
    if ((ch === 'h' || ch === 'w' || ch === 'H' || ch === 'W') && (i === 0 || /[\s(*_~]/.test(s[i - 1]))) {
      const m = BARE_URL.exec(s.slice(i));
      if (m) {
        let url = m[0].replace(/[.,;:!?)\]*_~]+$/, '');
        // Keep a closing ")" that belongs to the URL (Wikipedia-style links).
        while (m[0].length > url.length && s[i + url.length] === ')' && (url.match(/\(/g)?.length ?? 0) > (url.match(/\)/g)?.length ?? 0)) url += ')';
        const href = safeHref(/^www\./i.test(url) ? `https://${url}` : url);
        if (href) {
          out.push({ t: 'link', href, c: [{ t: 'text', v: url }] });
          i += url.length;
          continue;
        }
      }
    }
    const mark =
      s.startsWith('**', i) || s.startsWith('__', i)
        ? s.slice(i, i + 2)
        : s.startsWith('~~', i)
          ? '~~'
          : ch === '*' || ch === '_'
            ? ch
            : null;
    if (mark && s[i + mark.length] && !/\s/.test(s[i + mark.length]) && !(mark[0] === '_' && i > 0 && /\w/.test(s[i - 1]))) {
      const end = findClose(s, i + mark.length, mark);
      if (end > 0) {
        const c = parseInline(s.slice(i + mark.length, end));
        out.push(mark === '~~' ? { t: 'del', c } : mark.length === 2 ? { t: 'strong', c } : { t: 'em', c });
        i = end + mark.length;
        continue;
      }
    }
    pushText(out, ch);
    i++;
  }
  return out;
}

/** Plain text of inline nodes (for copying a table cell, tests). */
export function inlineText(nodes: Inline[]): string {
  return nodes.map((n) => (n.t === 'text' || n.t === 'code' ? n.v : n.t === 'br' ? '\n' : inlineText(n.c))).join('');
}
