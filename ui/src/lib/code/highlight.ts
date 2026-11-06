// TypeScript syntax highlighting for the read-only code views (generated scripts, heal patches).
// Prism core + the TypeScript grammar (~8 kB gzip), loaded on demand by CodeView.svelte so the
// chrome's main bundle does not carry it. Prism.highlight escapes the code, so the HTML is safe
// to insert.

import './prism-manual';
import Prism from 'prismjs';
import 'prismjs/components/prism-typescript';

export function highlightTs(code: string): string {
  return Prism.highlight(code, Prism.languages.typescript, 'typescript');
}
