// Small sRGB helpers for the theme system: WCAG 2.x relative luminance and contrast ratio,
// and the ink (near-black or white) that reads best on a fill. Pure, no DOM.

export type Rgb = [r: number, g: number, b: number];

/** `#rgb` / `#rrggbb` → [r, g, b] in 0..255, or null for anything else. */
export function parseHex(hex: string): Rgb | null {
  const h = hex.trim().replace(/^#/, '');
  const full = h.length === 3 ? [...h].map((c) => c + c).join('') : h;
  if (!/^[0-9a-f]{6}$/i.test(full)) return null;
  return [0, 2, 4].map((i) => parseInt(full.slice(i, i + 2), 16)) as Rgb;
}

export function toHex([r, g, b]: Rgb): string {
  return '#' + [r, g, b].map((v) => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, '0')).join('');
}

/** WCAG relative luminance (0 = black, 1 = white); 0 for an unparsable colour. */
export function luminance(hex: string): number {
  const rgb = parseHex(hex);
  if (!rgb) return 0;
  const [r, g, b] = rgb.map((v) => {
    const c = v / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast ratio between two opaque colours, 1..21. */
export function contrast(a: string, b: string): number {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

export const INK_DARK = '#111318';
export const INK_LIGHT = '#ffffff';

/** The text colour (near-black or white) with the higher contrast on `fill`. */
export function inkOn(fill: string, dark = INK_DARK, light = INK_LIGHT): string {
  return contrast(fill, dark) >= contrast(fill, light) ? dark : light;
}

/** `a` mixed with `b` (t = 0 → a, 1 → b), in sRGB. */
export function mix(a: string, b: string, t: number): string {
  const x = parseHex(a);
  const y = parseHex(b);
  if (!x || !y) return a;
  return toHex([0, 1, 2].map((i) => x[i] + (y[i] - x[i]) * t) as Rgb);
}
