// Sentences with markup inside (a path in <code>, a JSON shape in a mono span): one dictionary
// string with `{slot}` placeholders instead of fragments around the markup, so each language
// keeps its own word order. The component renders the text parts and puts its markup at the
// slots: {#each richParts(t('key')) as p}{#if p.slot === 'dir'}<code>…</code>{:else}{p.text}{/if}{/each}

export interface RichPart {
  /** Plain text (empty for a slot). */
  text: string;
  /** The placeholder's name, or null for plain text. */
  slot: string | null;
}

/** "Plugins are in {dir}." → text "Plugins are in ", slot "dir", text ".". */
export function richParts(text: string): RichPart[] {
  const parts: RichPart[] = [];
  let last = 0;
  for (const m of text.matchAll(/\{(\w+)\}/g)) {
    if (m.index > last) parts.push({ text: text.slice(last, m.index), slot: null });
    parts.push({ text: '', slot: m[1] });
    last = m.index + m[0].length;
  }
  if (last < text.length) parts.push({ text: text.slice(last), slot: null });
  return parts;
}
