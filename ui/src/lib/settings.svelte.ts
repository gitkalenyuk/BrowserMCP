import { load, save, watch } from './storage';
import { inkOn, parseHex } from './theme/color';
import { DEFAULT_PALETTE, isPaletteId, paletteById, TOKEN_NAMES, type Mode, type PaletteId, type Tokens } from './theme/palettes';

export type ThemePref = 'system' | 'light' | 'dark';
export type Density = 'comfortable' | 'compact';
export type TextSize = 'small' | 'default' | 'large';
export type PanelId = 'traffic' | 'recording' | 'ai' | 'terminal' | 'scripts' | 'runs';
export type WidgetId = 'prompt' | 'agents' | 'scripts' | 'sites';

export const ACCENTS = ['#3b82f6', '#8b5cf6', '#ec4899', '#ef4444', '#f97316', '#eab308', '#10b981', '#06b6d4'] as const;
/** `accent` value meaning "the palette's own accent" (any other value is a custom #rrggbb). */
export const PALETTE_ACCENT = 'palette';
/** The fixed accent every install had before palettes; untouched, it now means "palette". */
const LEGACY_DEFAULT_ACCENT = '#3b82f6';

export interface Settings {
  /** UI language (`system` = the browser's preferred languages). See `lib/i18n`. */
  language: 'system' | 'en' | 'uk';
  /** Light / dark mode (`system` follows Windows). */
  theme: ThemePref;
  /** Colour theme (lib/theme/palettes.ts). */
  palette: PaletteId;
  /** `palette` (the palette's own accent) or a custom `#rrggbb`. */
  accent: string;
  /** Compact: shorter tab strip, toolbar and tabs. */
  density: Density;
  /** Interface text size. */
  textSize: TextSize;
  verticalTabs: boolean;
  bookmarksBar: boolean;
  sidebarWidth: number;
  panel: PanelId | null;
  panelWidth: number;
  widgets: Record<WidgetId, boolean>;
}

export const DEFAULT_SETTINGS: Settings = {
  language: 'system',
  theme: 'system',
  palette: DEFAULT_PALETTE,
  accent: PALETTE_ACCENT,
  density: 'comfortable',
  textSize: 'default',
  verticalTabs: false,
  bookmarksBar: true,
  sidebarWidth: 240,
  panel: import.meta.env.VITE_BMCP_PANEL ?? null,
  panelWidth: 380,
  widgets: { prompt: true, agents: true, scripts: true, sites: true },
};

const KEY = 'settings';

/** Stored settings of any age → current settings; unknown values fall back to the defaults. */
export function migrateSettings(stored: Partial<Settings>): Settings {
  const out: Settings = {
    ...DEFAULT_SETTINGS,
    ...stored,
    widgets: { ...DEFAULT_SETTINGS.widgets, ...(stored.widgets ?? {}) },
  };
  if (!['system', 'light', 'dark'].includes(out.theme)) out.theme = DEFAULT_SETTINGS.theme;
  if (!isPaletteId(out.palette)) out.palette = DEFAULT_PALETTE;
  // Before palettes the accent was always a colour: the untouched old default becomes "palette".
  if (!('palette' in stored) && typeof stored.accent === 'string' && stored.accent.toLowerCase() === LEGACY_DEFAULT_ACCENT) {
    out.accent = PALETTE_ACCENT;
  }
  if (out.accent !== PALETTE_ACCENT && !parseHex(String(out.accent))) out.accent = PALETTE_ACCENT;
  if (!['small', 'default', 'large'].includes(out.textSize)) out.textSize = DEFAULT_SETTINGS.textSize;
  return out;
}

function read(): Settings {
  return migrateSettings(load<Partial<Settings>>(KEY, {}));
}

/** The palette variant for `mode` with the settings' accent override applied. */
export function themeTokens(s: Pick<Settings, 'palette' | 'accent'>, mode: Mode): Tokens {
  const base = paletteById(s.palette)[mode];
  if (s.accent === PALETTE_ACCENT || !parseHex(s.accent)) return base;
  // Custom accent: buttons, focus rings and links take it; the text on it is the ink that reads best.
  return { ...base, accent: s.accent, 'on-accent': inkOn(s.accent), focus: s.accent };
}

/** Persisted UI preferences shared by every page of the UI (same origin, synced via storage events). */
class SettingsStore {
  value = $state<Settings>(read());
  #systemDark = $state(false);

  constructor() {
    if (typeof window === 'undefined') return;
    const mq = window.matchMedia?.('(prefers-color-scheme: dark)');
    if (mq) {
      this.#systemDark = mq.matches;
      mq.addEventListener('change', (e) => (this.#systemDark = e.matches));
    }
    watch(KEY, () => (this.value = read()));
  }

  /** Light or dark for a mode preference (reactive: `system` follows Windows). */
  modeFor(pref: ThemePref): Mode {
    if (pref === 'system') return this.#systemDark ? 'dark' : 'light';
    return pref;
  }

  /** The theme actually shown. */
  get effectiveTheme(): Mode {
    return this.modeFor(this.value.theme);
  }

  /** The colour tokens shown now (palette variant in the current mode + accent override). */
  get tokens(): Tokens {
    return themeTokens(this.value, this.effectiveTheme);
  }

  /** The accent shown now, as #rrggbb. */
  get effectiveAccent(): string {
    return this.tokens.accent;
  }

  update(patch: Partial<Settings>): void {
    this.value = { ...this.value, ...patch };
    save(KEY, this.value);
  }

  toggleTheme(): void {
    this.update({ theme: this.effectiveTheme === 'dark' ? 'light' : 'dark' });
  }

  togglePanel(id: PanelId): void {
    this.update({ panel: this.value.panel === id ? null : id });
  }
}

export const settings = new SettingsStore();

/** Applies palette, mode, accent, density and text size to :root. Call inside an $effect. */
export function applyTheme(root: HTMLElement, s: Settings): void {
  const mode = settings.modeFor(s.theme);
  const tokens = themeTokens(s, mode);
  for (const name of TOKEN_NAMES) root.style.setProperty(`--${name}`, tokens[name]);
  root.style.colorScheme = mode;
  root.dataset.theme = mode;
  root.dataset.palette = s.palette;
  if (s.density === 'compact') root.dataset.density = 'compact';
  else delete root.dataset.density;
  if (s.textSize && s.textSize !== 'default') root.dataset.textSize = s.textSize;
  else delete root.dataset.textSize;
}
