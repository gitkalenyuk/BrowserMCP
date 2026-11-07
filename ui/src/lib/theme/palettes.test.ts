import { describe, expect, it } from 'vitest';
import { contrast, inkOn, parseHex } from './color';
import { GROUP_NAMES, PALETTES, PALETTE_IDS, TOKEN_NAMES, type Mode, type Palette, type TokenName } from './palettes';

// WCAG 2.x: 4.5:1 for normal text (AA), 7:1 (AAA); 3:1 for large text and UI components.
const TEXT_AA = 4.5;
const TEXT_AAA = 7;
const UI = 3;

const BACKGROUNDS: TokenName[] = ['frame', 'toolbar', 'surface', 'sunken', 'field', 'field-focus'];

/** [foreground, background, minimum ratio] pairs the UI actually draws. */
function pairs(p: Palette): [TokenName, TokenName, number][] {
  const text = p.level === 'AAA' ? TEXT_AAA : TEXT_AA;
  const out: [TokenName, TokenName, number][] = [];
  for (const bg of BACKGROUNDS) out.push(['fg', bg, text]);
  out.push(['fg', 'selection', text]);
  for (const fg of ['fg-muted', 'fg-subtle'] as const) for (const bg of ['frame', 'toolbar', 'surface', 'sunken'] as const) out.push([fg, bg, text]);
  out.push(['on-accent', 'accent', text]);
  out.push(['accent', 'surface', text], ['accent', 'toolbar', UI], ['accent', 'sunken', UI]);
  for (const s of ['danger', 'ok', 'warn'] as const) for (const bg of ['surface', 'toolbar', 'sunken'] as const) out.push([s, bg, text]);
  out.push(['on-danger', 'danger', text]);
  out.push(['code-fg', 'code-bg', text]);
  for (const bg of ['surface', 'toolbar', 'frame'] as const) out.push(['focus', bg, UI]);
  for (const g of GROUP_NAMES) {
    // Group names are written in the group colour on the strip and the sidebar…
    for (const bg of ['frame', 'toolbar', 'surface'] as const) out.push([`g-${g}`, bg, text]);
    // …and in `on-group` on a filled chip.
    out.push(['on-group', `g-${g}`, text]);
  }
  if (p.level === 'AAA') for (const bg of ['surface', 'toolbar', 'frame'] as const) out.push(['border', bg, UI]);
  return out;
}

describe('colour helpers', () => {
  it('computes WCAG contrast', () => {
    expect(contrast('#000000', '#ffffff')).toBeCloseTo(21, 5);
    expect(contrast('#ffffff', '#ffffff')).toBeCloseTo(1, 5);
    // The WCAG reference value for #777 on white is 4.48.
    expect(contrast('#777777', '#ffffff')).toBeCloseTo(4.48, 2);
  });

  it('picks the more readable ink', () => {
    expect(inkOn('#ffd400')).toBe('#111318');
    expect(inkOn('#1d4ed8')).toBe('#ffffff');
  });
});

describe('palettes', () => {
  it('has unique ids, one entry per id', () => {
    expect(PALETTES.map((p) => p.id)).toEqual([...PALETTE_IDS]);
    expect(PALETTES.length).toBeGreaterThanOrEqual(8);
  });

  it('define every token as an opaque hex colour in both variants', () => {
    for (const p of PALETTES) {
      for (const mode of ['light', 'dark'] as Mode[]) {
        for (const name of TOKEN_NAMES) {
          const v = p[mode][name];
          expect(parseHex(v ?? ''), `${p.id}.${mode}.${name} = ${v}`).not.toBeNull();
        }
      }
    }
  });

  it('light variants are light and dark variants are dark', () => {
    for (const p of PALETTES) {
      expect(contrast(p.light.surface, '#000000'), `${p.id} light`).toBeGreaterThan(contrast(p.light.surface, '#ffffff'));
      expect(contrast(p.dark.surface, '#ffffff'), `${p.id} dark`).toBeGreaterThan(contrast(p.dark.surface, '#000000'));
    }
  });

  it('every text / background and accent / ink pair meets WCAG contrast', () => {
    const failures: string[] = [];
    for (const p of PALETTES) {
      for (const mode of ['light', 'dark'] as Mode[]) {
        const tk = p[mode];
        for (const [fg, bg, min] of pairs(p)) {
          const ratio = contrast(tk[fg], tk[bg]);
          if (ratio < min) failures.push(`${p.id}.${mode}: ${fg} ${tk[fg]} on ${bg} ${tk[bg]} = ${ratio.toFixed(2)} < ${min}`);
        }
      }
    }
    expect(failures).toEqual([]);
  });
});
