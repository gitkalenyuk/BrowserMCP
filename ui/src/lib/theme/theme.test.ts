import css from '../styles/app.css?raw';
import { describe, expect, it } from 'vitest';
import { DEFAULT_SETTINGS, migrateSettings, PALETTE_ACCENT, themeTokens } from '../settings.svelte';
import { contrast } from './color';
import { paletteById, TOKEN_NAMES } from './palettes';

describe('settings migration', () => {
  it('turns the old fixed default accent into "palette default"', () => {
    const s = migrateSettings({ theme: 'dark', accent: '#3b82f6' });
    expect(s.palette).toBe('graphite');
    expect(s.accent).toBe(PALETTE_ACCENT);
    expect(s.theme).toBe('dark');
  });

  it('keeps a custom accent the user picked before palettes existed', () => {
    expect(migrateSettings({ accent: '#ec4899' }).accent).toBe('#ec4899');
  });

  it('keeps #3b82f6 once it was picked with palettes around', () => {
    expect(migrateSettings({ palette: 'forest', accent: '#3b82f6' }).accent).toBe('#3b82f6');
  });

  it('falls back on unknown values', () => {
    const s = migrateSettings({ palette: 'nope' as never, accent: 'red', theme: 'sepia' as never, textSize: 'huge' as never });
    expect(s.palette).toBe(DEFAULT_SETTINGS.palette);
    expect(s.accent).toBe(PALETTE_ACCENT);
    expect(s.theme).toBe('system');
    expect(s.textSize).toBe('default');
  });
});

describe('theme tokens', () => {
  it('uses the palette accent by default', () => {
    expect(themeTokens({ palette: 'lagoon', accent: PALETTE_ACCENT }, 'dark')).toEqual(paletteById('lagoon').dark);
  });

  it('a custom accent replaces accent and focus and gets a readable ink', () => {
    for (const accent of ['#eab308', '#1d4ed8', '#10b981', '#ffffff', '#000000']) {
      const tk = themeTokens({ palette: 'paper', accent }, 'light');
      expect(tk.accent).toBe(accent);
      expect(tk.focus).toBe(accent);
      expect(contrast(tk['on-accent'], accent)).toBeGreaterThanOrEqual(4.5);
      expect(tk.surface).toBe(paletteById('paper').light.surface);
    }
  });
});

describe('app.css', () => {
  const block = (selector: string): Record<string, string> => {
    const start = css.indexOf(selector);
    const body = css.slice(css.indexOf('{', start) + 1, css.indexOf('}', start));
    return Object.fromEntries([...body.matchAll(/--([a-z0-9-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
  };

  it('pre-script defaults are the Graphite palette (light and dark)', () => {
    const light = block(':root {');
    const dark = block(":root:not([data-theme='light'])");
    const g = paletteById('graphite');
    for (const name of TOKEN_NAMES) {
      expect(light[name], `light --${name}`).toBe(g.light[name]);
      expect(dark[name], `dark --${name}`).toBe(g.dark[name]);
    }
  });
});
