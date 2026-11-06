import { load, save, watch } from './storage';

export type ThemePref = 'system' | 'light' | 'dark';
export type PanelId = 'traffic' | 'recording' | 'ai' | 'terminal' | 'scripts' | 'runs';
export type WidgetId = 'prompt' | 'agents' | 'scripts' | 'sites';

export const ACCENTS = ['#3b82f6', '#8b5cf6', '#ec4899', '#ef4444', '#f97316', '#eab308', '#10b981', '#06b6d4'] as const;

export interface Settings {
  theme: ThemePref;
  accent: string;
  verticalTabs: boolean;
  bookmarksBar: boolean;
  sidebarWidth: number;
  panel: PanelId | null;
  panelWidth: number;
  widgets: Record<WidgetId, boolean>;
}

export const DEFAULT_SETTINGS: Settings = {
  theme: 'system',
  accent: '#3b82f6',
  verticalTabs: false,
  bookmarksBar: true,
  sidebarWidth: 240,
  panel: import.meta.env.VITE_BMCP_PANEL ?? null,
  panelWidth: 380,
  widgets: { prompt: true, agents: true, scripts: true, sites: true },
};

const KEY = 'settings';

function read(): Settings {
  const stored = load<Partial<Settings>>(KEY, {});
  return {
    ...DEFAULT_SETTINGS,
    ...stored,
    widgets: { ...DEFAULT_SETTINGS.widgets, ...(stored.widgets ?? {}) },
  };
}

/** Persisted UI preferences shared by the chrome and the new tab page (same origin). */
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

  /** The theme actually shown. */
  get effectiveTheme(): 'light' | 'dark' {
    if (this.value.theme === 'system') return this.#systemDark ? 'dark' : 'light';
    return this.value.theme;
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

function luminance(hex: string): number {
  const m = hex.replace('#', '').match(/^([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i);
  if (!m) return 0;
  const [r, g, b] = m.slice(1).map((h) => {
    const c = parseInt(h, 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Applies theme + accent to :root. Call inside an $effect. */
export function applyTheme(root: HTMLElement, s: Settings): void {
  if (s.theme === 'system') delete root.dataset.theme;
  else root.dataset.theme = s.theme;
  root.style.setProperty('--accent', s.accent);
  // Text on accent fills: dark ink on light accents, white otherwise.
  root.style.setProperty('--on-accent', luminance(s.accent) > 0.4 ? '#111318' : '#ffffff');
}
