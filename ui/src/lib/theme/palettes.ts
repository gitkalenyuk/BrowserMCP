// Colour themes (Settings → General → Appearance). A palette is a complete set of the colour
// tokens the UI paints with, in a light and a dark variant; `applyTheme` (settings.svelte.ts)
// writes the chosen variant onto :root as CSS custom properties (`--frame`, `--fg`…).
//
// Tokens that only depend on light / dark (hover and press veils, scrim, shadows) stay in
// lib/styles/app.css and are derived from these. palettes.test.ts checks WCAG contrast of
// every text / background and accent / ink pair, so a palette that is hard to read fails CI.

export const PALETTE_IDS = ['graphite', 'midnight', 'forest', 'ember', 'rosewood', 'lagoon', 'dusk', 'paper', 'contrast'] as const;
export type PaletteId = (typeof PALETTE_IDS)[number];
export type Mode = 'light' | 'dark';

/** Tab group / agent colours (`--g-<name>`), the GroupColor of the API. */
export const GROUP_NAMES = ['grey', 'blue', 'red', 'yellow', 'green', 'pink', 'purple', 'cyan', 'orange'] as const;
export type GroupName = (typeof GROUP_NAMES)[number];

/** Every colour token a palette sets, named as the CSS custom property without `--`. */
export const TOKEN_NAMES = [
  'frame', // window frame and tab strip
  'toolbar', // toolbar, panel headers, the selected tab
  'surface', // cards, popups, pages
  'sunken', // wells, segmented controls, inactive areas
  'field', // inputs at rest
  'field-focus', // inputs with focus
  'fg', // body text
  'fg-muted', // secondary text
  'fg-subtle', // hints, captions
  'border',
  'border-strong',
  'accent', // the palette's own accent (a custom accent in settings replaces it)
  'on-accent', // text on accent fills
  'focus', // focus rings
  'selection', // selected text background
  'danger',
  'on-danger',
  'ok',
  'warn',
  'code-bg', // terminal and code areas
  'code-fg',
  'scrollbar', // scrollbar thumb
  ...GROUP_NAMES.map((g) => `g-${g}` as const),
  'on-group', // text on a group-coloured fill
] as const;
export type TokenName = (typeof TOKEN_NAMES)[number];
export type Tokens = Record<TokenName, string>;

export interface Palette {
  id: PaletteId;
  /** WCAG level the palette is tested against: AA (4.5:1 text) or AAA (7:1 text). */
  level: 'AA' | 'AAA';
  light: Tokens;
  dark: Tokens;
}

type Groups = Record<`g-${GroupName}` | 'on-group', string>;

// Group colours readable as text on every light / dark strip and as fills under `on-group`.
const GROUPS_LIGHT: Groups = {
  'g-grey': '#575e6a',
  'g-blue': '#1a5fd0',
  'g-red': '#b81f2a',
  'g-yellow': '#7f5500',
  'g-green': '#166b2e',
  'g-pink': '#a82f78',
  'g-purple': '#7140c8',
  'g-cyan': '#0b6a77',
  'g-orange': '#a8461a',
  'on-group': '#ffffff',
};

const GROUPS_DARK: Groups = {
  'g-grey': '#a3a9b4',
  'g-blue': '#74adff',
  'g-red': '#ff8279',
  'g-yellow': '#e5b84a',
  'g-green': '#5fd46c',
  'g-pink': '#f882bf',
  'g-purple': '#c197ff',
  'g-cyan': '#55d3df',
  'g-orange': '#ff9f66',
  'on-group': '#111318',
};

type Base = Omit<Tokens, keyof Groups | 'focus' | 'scrollbar'> & Partial<Pick<Tokens, 'focus' | 'scrollbar'>>;

function light(base: Base, groups: Partial<Groups> = {}): Tokens {
  return { focus: base.accent, scrollbar: base['border-strong'], ...base, ...GROUPS_LIGHT, ...groups } as Tokens;
}

function dark(base: Base, groups: Partial<Groups> = {}): Tokens {
  return { focus: base.accent, scrollbar: base['border-strong'], ...base, ...GROUPS_DARK, ...groups } as Tokens;
}

export const PALETTES: Palette[] = [
  {
    // The original BrowserMCP look: neutral greys, blue accent.
    id: 'graphite',
    level: 'AA',
    light: light({
      frame: '#e6e8ec',
      toolbar: '#f7f8fa',
      surface: '#ffffff',
      sunken: '#eceef2',
      field: '#eceef2',
      'field-focus': '#ffffff',
      fg: '#16191f',
      'fg-muted': '#4f5664',
      'fg-subtle': '#5f6673',
      border: '#d8dbe1',
      'border-strong': '#c1c6cf',
      accent: '#2563eb',
      'on-accent': '#ffffff',
      selection: '#c6dafc',
      danger: '#c4314b',
      'on-danger': '#ffffff',
      ok: '#17754a',
      warn: '#8f5d00',
      'code-bg': '#ffffff',
      'code-fg': '#1f2328',
    }),
    dark: dark({
      frame: '#131519',
      toolbar: '#1e2126',
      surface: '#24272d',
      sunken: '#191b1f',
      field: '#131519',
      'field-focus': '#17191d',
      fg: '#e7e9ed',
      'fg-muted': '#aab0bb',
      'fg-subtle': '#959ba6',
      border: '#2f333a',
      'border-strong': '#3d424a',
      accent: '#5e9bff',
      'on-accent': '#0d1016',
      selection: '#264f78',
      danger: '#ff7b8a',
      'on-danger': '#16080b',
      ok: '#4ccb7f',
      warn: '#e3b341',
      'code-bg': '#0d0f12',
      'code-fg': '#c9d1d9',
    }),
  },
  {
    // Deep navy with a periwinkle accent.
    id: 'midnight',
    level: 'AA',
    light: light({
      frame: '#dfe5f2',
      toolbar: '#f3f6fc',
      surface: '#ffffff',
      sunken: '#e8edf7',
      field: '#e8edf7',
      'field-focus': '#ffffff',
      fg: '#0f1730',
      'fg-muted': '#3e4a66',
      'fg-subtle': '#525e7a',
      border: '#ccd5e6',
      'border-strong': '#b0bcd4',
      accent: '#3550d8',
      'on-accent': '#ffffff',
      selection: '#cbd7ff',
      danger: '#c12b48',
      'on-danger': '#ffffff',
      ok: '#157a4a',
      warn: '#8a5a00',
      'code-bg': '#f6f8fd',
      'code-fg': '#1a2440',
    }),
    dark: dark({
      frame: '#0b1020',
      toolbar: '#121a2e',
      surface: '#172036',
      sunken: '#0e1526',
      field: '#0b1020',
      'field-focus': '#0f1729',
      fg: '#e6ebf5',
      'fg-muted': '#a9b4cc',
      'fg-subtle': '#919db6',
      border: '#243050',
      'border-strong': '#33416a',
      accent: '#7ea5ff',
      'on-accent': '#0b1020',
      selection: '#2b3d6b',
      danger: '#ff8597',
      'on-danger': '#1a070c',
      ok: '#5fd39a',
      warn: '#f0c05a',
      'code-bg': '#080c18',
      'code-fg': '#d5ddf0',
    }),
  },
  {
    // Moss and pine, a green accent.
    id: 'forest',
    level: 'AA',
    light: light({
      frame: '#dfe6e0',
      toolbar: '#f3f6f3',
      surface: '#fcfdfb',
      sunken: '#e7ece7',
      field: '#e7ece7',
      'field-focus': '#ffffff',
      fg: '#142019',
      'fg-muted': '#43554a',
      'fg-subtle': '#53655a',
      border: '#cdd8cf',
      'border-strong': '#b3c2b7',
      accent: '#1d7346',
      'on-accent': '#ffffff',
      selection: '#c7e5d2',
      danger: '#b8323f',
      'on-danger': '#ffffff',
      ok: '#1a7038',
      warn: '#855600',
      'code-bg': '#f5f8f5',
      'code-fg': '#18261d',
    }),
    dark: dark({
      frame: '#0f1512',
      toolbar: '#18201b',
      surface: '#1d2621',
      sunken: '#131a16',
      field: '#0f1512',
      'field-focus': '#121915',
      fg: '#e4ece6',
      'fg-muted': '#a7b8ad',
      'fg-subtle': '#91a297',
      border: '#2a372f',
      'border-strong': '#3a4a40',
      accent: '#5cc98a',
      'on-accent': '#0b130e',
      selection: '#2c4a38',
      danger: '#ff8a80',
      'on-danger': '#1a0806',
      ok: '#7bd88f',
      warn: '#e6c15a',
      'code-bg': '#0b100d',
      'code-fg': '#d3e2d8',
    }),
  },
  {
    // Sunset: warm browns, an amber-orange accent.
    id: 'ember',
    level: 'AA',
    light: light({
      frame: '#efe3d6',
      toolbar: '#faf4ee',
      surface: '#fffcf8',
      sunken: '#f3e9de',
      field: '#f3e9de',
      'field-focus': '#ffffff',
      fg: '#2a1a0f',
      'fg-muted': '#5e4a3b',
      'fg-subtle': '#6e5949',
      border: '#e3d3c2',
      'border-strong': '#cdb9a4',
      accent: '#ad460d',
      'on-accent': '#ffffff',
      selection: '#fbd3ae',
      danger: '#b42d35',
      'on-danger': '#ffffff',
      ok: '#277038',
      warn: '#865200',
      'code-bg': '#fbf6f0',
      'code-fg': '#2e1f14',
    }),
    dark: dark({
      frame: '#17120e',
      toolbar: '#211a15',
      surface: '#281f19',
      sunken: '#1b1511',
      field: '#17120e',
      'field-focus': '#1a1410',
      fg: '#f1e7de',
      'fg-muted': '#c4b2a3',
      'fg-subtle': '#aa9888',
      border: '#3a2e25',
      'border-strong': '#4b3c31',
      accent: '#ff9a4d',
      'on-accent': '#1a0f06',
      selection: '#5a3b22',
      danger: '#ff8080',
      'on-danger': '#1a0707',
      ok: '#79d48b',
      warn: '#f2c14e',
      'code-bg': '#110d0a',
      'code-fg': '#ead9ca',
    }),
  },
  {
    // Dusty rose and plum, a raspberry accent.
    id: 'rosewood',
    level: 'AA',
    light: light({
      frame: '#f1e2e8',
      toolbar: '#fbf4f7',
      surface: '#fffbfd',
      sunken: '#f5e8ee',
      field: '#f5e8ee',
      'field-focus': '#ffffff',
      fg: '#2a1320',
      'fg-muted': '#5f4553',
      'fg-subtle': '#6f5563',
      border: '#e7d1db',
      'border-strong': '#d3b5c3',
      accent: '#b82866',
      'on-accent': '#ffffff',
      selection: '#f8c6db',
      danger: '#b3261e',
      'on-danger': '#ffffff',
      ok: '#1c7040',
      warn: '#865600',
      'code-bg': '#fdf6f9',
      'code-fg': '#2e1722',
    }),
    dark: dark({
      frame: '#171115',
      toolbar: '#21191e',
      surface: '#281e24',
      sunken: '#1b1418',
      field: '#171115',
      'field-focus': '#1a1317',
      fg: '#f1e6ec',
      'fg-muted': '#c6b0bc',
      'fg-subtle': '#ad97a4',
      border: '#3a2a33',
      'border-strong': '#4c3843',
      accent: '#ff80b2',
      'on-accent': '#1d0a12',
      selection: '#5a2c42',
      danger: '#ff8a7a',
      'on-danger': '#1a0905',
      ok: '#74d39a',
      warn: '#efc15a',
      'code-bg': '#120d10',
      'code-fg': '#ecd9e3',
    }),
  },
  {
    // Ocean: blue-green water, a teal accent.
    id: 'lagoon',
    level: 'AA',
    light: light({
      frame: '#dbe8ea',
      toolbar: '#f1f7f8',
      surface: '#fbfefe',
      sunken: '#e3eef0',
      field: '#e3eef0',
      'field-focus': '#ffffff',
      fg: '#0e2326',
      'fg-muted': '#3a5a5e',
      'fg-subtle': '#4a6a6e',
      border: '#c6dadd',
      'border-strong': '#a9c3c7',
      accent: '#0a7270',
      'on-accent': '#ffffff',
      selection: '#b9e6e4',
      danger: '#b8303d',
      'on-danger': '#ffffff',
      ok: '#187a43',
      warn: '#835700',
      'code-bg': '#f3f9fa',
      'code-fg': '#13292c',
    }),
    dark: dark({
      frame: '#0b1517',
      toolbar: '#121f22',
      surface: '#162629',
      sunken: '#0f1a1c',
      field: '#0b1517',
      'field-focus': '#0e191b',
      fg: '#e2eff0',
      'fg-muted': '#a3bec1',
      'fg-subtle': '#8aa6a9',
      border: '#223a3e',
      'border-strong': '#2f4d52',
      accent: '#3fd0c9',
      'on-accent': '#04201e',
      selection: '#1d4d50',
      danger: '#ff8a8a',
      'on-danger': '#1a0808',
      ok: '#6edb9a',
      warn: '#edc45c',
      'code-bg': '#081012',
      'code-fg': '#cfe5e7',
    }),
  },
  {
    // Twilight violet.
    id: 'dusk',
    level: 'AA',
    light: light({
      frame: '#e6e2f1',
      toolbar: '#f6f4fb',
      surface: '#fefdff',
      sunken: '#eeebf6',
      field: '#eeebf6',
      'field-focus': '#ffffff',
      fg: '#1c1630',
      'fg-muted': '#4a4266',
      'fg-subtle': '#5a5276',
      border: '#d8d2e8',
      'border-strong': '#bfb6d6',
      accent: '#6a3fd1',
      'on-accent': '#ffffff',
      selection: '#ddd0ff',
      danger: '#bb2a48',
      'on-danger': '#ffffff',
      ok: '#1d7a48',
      warn: '#865700',
      'code-bg': '#f8f6fd',
      'code-fg': '#211a36',
    }),
    dark: dark({
      frame: '#131120',
      toolbar: '#1c192c',
      surface: '#221e34',
      sunken: '#171427',
      field: '#131120',
      'field-focus': '#161325',
      fg: '#ebe7f6',
      'fg-muted': '#b5adcc',
      'fg-subtle': '#9d95b5',
      border: '#2f2a46',
      'border-strong': '#3f3859',
      accent: '#a98efa',
      'on-accent': '#150d2b',
      selection: '#3e3270',
      danger: '#ff8597',
      'on-danger': '#1a070c',
      ok: '#6fd6a0',
      warn: '#eec35a',
      'code-bg': '#0e0c18',
      'code-fg': '#ddd6f0',
    }),
  },
  {
    // Warm paper and ink: soft contrast without glare, for long reading sessions.
    id: 'paper',
    level: 'AA',
    light: light({
      frame: '#ebe4d6',
      toolbar: '#f6f1e7',
      surface: '#fdfaf3',
      sunken: '#efe8da',
      field: '#efe8da',
      'field-focus': '#fffdf8',
      fg: '#1f1a12',
      'fg-muted': '#4a4131',
      'fg-subtle': '#5a503e',
      border: '#ddd3bf',
      'border-strong': '#c7bba2',
      accent: '#1f5c99',
      'on-accent': '#ffffff',
      selection: '#f2dfa8',
      danger: '#a8281f',
      'on-danger': '#ffffff',
      ok: '#2c6e2f',
      warn: '#7a5000',
      'code-bg': '#f7f2e6',
      'code-fg': '#261f15',
    }),
    dark: dark({
      frame: '#1a1712',
      toolbar: '#24201a',
      surface: '#2a251e',
      sunken: '#1e1b15',
      field: '#1a1712',
      'field-focus': '#1d1a14',
      fg: '#efe6d4',
      'fg-muted': '#c8bca5',
      'fg-subtle': '#afa38d',
      border: '#3b342a',
      'border-strong': '#4c4436',
      accent: '#8fb8e8',
      'on-accent': '#14100a',
      selection: '#4d4128',
      danger: '#f08a7e',
      'on-danger': '#1a0906',
      ok: '#8fcf8a',
      warn: '#e8c06a',
      'code-bg': '#15120e',
      'code-fg': '#e8dcc6',
    }),
  },
  {
    // Accessibility: pure black / white, strong borders, AAA text everywhere.
    id: 'contrast',
    level: 'AAA',
    light: light(
      {
        frame: '#ffffff',
        toolbar: '#f2f2f2',
        surface: '#ffffff',
        sunken: '#ebebeb',
        field: '#ffffff',
        'field-focus': '#ffffff',
        fg: '#000000',
        'fg-muted': '#1f1f1f',
        'fg-subtle': '#2e2e2e',
        border: '#595959',
        'border-strong': '#000000',
        accent: '#0033cc',
        'on-accent': '#ffffff',
        selection: '#ffe066',
        danger: '#9e0000',
        'on-danger': '#ffffff',
        ok: '#00591f',
        warn: '#5c3b00',
        'code-bg': '#ffffff',
        'code-fg': '#000000',
        scrollbar: '#595959',
      },
      {
        'g-grey': '#3d3d3d',
        'g-blue': '#0033cc',
        'g-red': '#9e0000',
        'g-yellow': '#5c3b00',
        'g-green': '#00591f',
        'g-pink': '#8a004f',
        'g-purple': '#4b1aa8',
        'g-cyan': '#004f5c',
        'g-orange': '#7a2900',
        'on-group': '#ffffff',
      },
    ),
    dark: dark(
      {
        frame: '#000000',
        toolbar: '#0a0a0a',
        surface: '#000000',
        sunken: '#141414',
        field: '#000000',
        'field-focus': '#000000',
        fg: '#ffffff',
        'fg-muted': '#ebebeb',
        'fg-subtle': '#d6d6d6',
        border: '#a0a0a0',
        'border-strong': '#ffffff',
        accent: '#ffd400',
        'on-accent': '#000000',
        selection: '#1a3aa8',
        danger: '#ff8f8f',
        'on-danger': '#000000',
        ok: '#7cff9b',
        warn: '#ffcc4d',
        'code-bg': '#000000',
        'code-fg': '#ffffff',
        scrollbar: '#a0a0a0',
      },
      {
        'g-grey': '#d0d0d0',
        'g-blue': '#9cc3ff',
        'g-red': '#ffa3a3',
        'g-yellow': '#ffd84d',
        'g-green': '#8cffa8',
        'g-pink': '#ffa8d8',
        'g-purple': '#d4b8ff',
        'g-cyan': '#80f0ff',
        'g-orange': '#ffbb80',
        'on-group': '#000000',
      },
    ),
  },
];

export const DEFAULT_PALETTE: PaletteId = 'graphite';

export function paletteById(id: string | undefined): Palette {
  return PALETTES.find((p) => p.id === id) ?? PALETTES[0];
}

export function isPaletteId(id: unknown): id is PaletteId {
  return typeof id === 'string' && (PALETTE_IDS as readonly string[]).includes(id);
}
