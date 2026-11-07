// Full-text search over the guide of one language. The guide is small (a few hundred kB of
// text), so a linear scan per keystroke is fast enough and needs no index structure.

import { plainText } from './inline';
import type { Block, Chapter } from './model';

export interface SectionHit {
  section: string;
  title: string;
  /** Text around the first match, for the result list. */
  snippet: string;
  /** How many of the query words the section contains. */
  words: number;
}

export interface ChapterHit {
  chapter: string;
  title: string;
  summary: string;
  score: number;
  sections: SectionHit[];
}

/** Every piece of searchable text of a block (inline marks removed). */
export function blockText(b: Block): string[] {
  switch (b.type) {
    case 'p':
    case 'tip':
    case 'warning':
      return [plainText(b.text)];
    case 'steps':
    case 'list':
      return b.items.map(plainText);
    case 'code':
      return [b.code];
    case 'keys':
      return b.items.map((k) => ('keys' in k ? `${k.keys.join(' ')} ${plainText(k.text)}` : plainText(k.text)));
    case 'settings':
    case 'see':
      return b.text ? [plainText(b.text)] : [];
    case 'api':
      return [b.command, b.command.replace('.', '_'), ...(b.text ? [plainText(b.text)] : [])];
    case 'terms':
      return b.items.map((i) => `${plainText(i.term)} ${plainText(i.text)}`);
  }
}

const SNIPPET = 140;

/** ~140 characters of `text` around its first match of `word`, with … where it was cut. */
export function snippetAround(text: string, word: string): string {
  const at = text.toLowerCase().indexOf(word);
  if (at < 0 || text.length <= SNIPPET) return text.length <= SNIPPET ? text : `${text.slice(0, SNIPPET).trimEnd()}…`;
  const start = Math.max(0, at - 50);
  const end = Math.min(text.length, start + SNIPPET);
  return `${start > 0 ? '…' : ''}${text.slice(start, end).trim()}${end < text.length ? '…' : ''}`;
}

/**
 * Chapters that contain every word of the query (anywhere: titles, text, code, commands),
 * best first. A title match counts more than a match in the text.
 */
export function searchGuide(chapters: Chapter[], words: string[]): ChapterHit[] {
  if (!words.length) return [];
  const hits: ChapterHit[] = [];
  for (const ch of chapters) {
    const sections: SectionHit[] = [];
    const found = new Set<string>();
    let score = 0;
    for (const w of words) {
      if (ch.title.toLowerCase().includes(w)) {
        found.add(w);
        score += 10;
      }
      if (ch.summary.toLowerCase().includes(w)) {
        found.add(w);
        score += 3;
      }
    }
    for (const s of ch.sections) {
      const texts = s.blocks.flatMap(blockText);
      const all = [s.title, ...texts].join('\n').toLowerCase();
      const present = words.filter((w) => all.includes(w));
      if (!present.length) continue;
      present.forEach((w) => found.add(w));
      score += present.length + words.filter((w) => s.title.toLowerCase().includes(w)).length * 5;
      const first = texts.find((t) => t.toLowerCase().includes(present[0])) ?? s.title;
      sections.push({ section: s.id, title: s.title, snippet: snippetAround(first, present[0]), words: present.length });
    }
    if (found.size < words.length) continue;
    sections.sort((a, b) => b.words - a.words);
    hits.push({ chapter: ch.id, title: ch.title, summary: ch.summary, score, sections });
  }
  return hits.sort((a, b) => b.score - a.score);
}
