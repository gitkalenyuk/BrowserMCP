// Inline marks of guide text and search highlighting, as plain data for the Svelte renderer.

export type Mark = 'text' | 'code' | 'strong' | 'kbd';
export interface Span {
  mark: Mark;
  text: string;
}
/** A piece of a span's text; `hit` = it matches the search query. */
export interface Piece {
  text: string;
  hit: boolean;
}

// `code` first: backticks protect `**` and `[[` inside them.
const TOKEN = /`([^`]+)`|\*\*(.+?)\*\*|\[\[(.+?)\]\]/g;

/** "Press [[Ctrl+K]] and type `tabs`" → text / kbd / text / code spans. */
export function parseInline(text: string): Span[] {
  const out: Span[] = [];
  let last = 0;
  for (const m of text.matchAll(TOKEN)) {
    const at = m.index ?? 0;
    if (at > last) out.push({ mark: 'text', text: text.slice(last, at) });
    if (m[1] !== undefined) out.push({ mark: 'code', text: m[1] });
    else if (m[2] !== undefined) out.push({ mark: 'strong', text: m[2] });
    else out.push({ mark: 'kbd', text: m[3] });
    last = at + m[0].length;
  }
  if (last < text.length) out.push({ mark: 'text', text: text.slice(last) });
  return out;
}

/** The text without marks (for search and snippets). */
export function plainText(text: string): string {
  return parseInline(text)
    .map((s) => s.text)
    .join('');
}

/** Lower-case words of a search query (empty = no search). */
export function queryWords(query: string): string[] {
  return [
    ...new Set(
      query
        .toLowerCase()
        .split(/\s+/)
        .filter((w) => w.length > 0),
    ),
  ];
}

/** Split `text` into pieces, marking every case-insensitive occurrence of any word. */
export function highlight(text: string, words: string[]): Piece[] {
  if (!words.length || !text) return [{ text, hit: false }];
  const lower = text.toLowerCase();
  // Mark matched characters first, so overlapping words ("tab", "tabs") merge into one piece.
  const hit = new Array<boolean>(text.length).fill(false);
  for (const w of words) {
    for (let i = lower.indexOf(w); i !== -1; i = lower.indexOf(w, i + 1)) {
      for (let j = i; j < i + w.length && j < text.length; j++) hit[j] = true;
    }
  }
  const out: Piece[] = [];
  for (let i = 0; i < text.length; ) {
    let j = i;
    while (j < text.length && hit[j] === hit[i]) j++;
    out.push({ text: text.slice(i, j), hit: hit[i] });
    i = j;
  }
  return out;
}
