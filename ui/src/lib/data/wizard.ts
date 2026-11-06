// The Chrome import wizard as a pure state machine (the component only renders it and runs
// the two calls: import.chrome_detect, import.chrome).
//
//   detecting --detected--> choose --start--> running --finished--> done --back--> choose
//       |                    |   ^                |
//       +--detect_failed-----+   +----failed------+
//   choose/detecting --skip--> skipped
//
// Parts the user picks:
//   browsing  -> `bookmarks` and/or `history` from the chosen Chrome profile folder
//   passwords -> `passwords_csv` (a CSV exported from chrome://password-manager/settings)
//   cookies   -> `cookies_json` (a JSON export from a cookie extension)

import type { ChromeProfile, ImportChromeArgs, ImportPart, ImportPartResult } from '../api/types-data';

export type WizardPart = 'browsing' | 'passwords' | 'cookies';
export type WizardStep = 'detecting' | 'choose' | 'running' | 'done' | 'skipped';

export interface WizardState {
  step: WizardStep;
  profiles: ChromeProfile[];
  userDataDirs: string[];
  /** Detection failed (the other parts still work). */
  detectError: string | null;
  /** Path of the chosen profile. */
  profile: string | null;
  parts: Record<WizardPart, boolean>;
  passwordsCsv: string;
  cookiesJson: string;
  results: ImportPartResult[];
  /** The last run failed as a whole (bad params, disconnected). */
  error: string | null;
}

export type WizardEvent =
  | { type: 'detected'; profiles: ChromeProfile[]; userDataDirs: string[] }
  | { type: 'detect_failed'; error: string }
  | { type: 'select_profile'; path: string }
  | { type: 'toggle'; part: WizardPart; on: boolean }
  | { type: 'set_path'; part: 'passwords' | 'cookies'; path: string }
  | { type: 'start' }
  | { type: 'finished'; results: ImportPartResult[] }
  | { type: 'failed'; error: string }
  | { type: 'skip' }
  | { type: 'back' };

export function initialWizard(): WizardState {
  return {
    step: 'detecting',
    profiles: [],
    userDataDirs: [],
    detectError: null,
    profile: null,
    parts: { browsing: false, passwords: false, cookies: false },
    passwordsCsv: '',
    cookiesJson: '',
    results: [],
    error: null,
  };
}

const hasData = (p: ChromeProfile) => p.has_bookmarks || p.has_history;

export function selectedProfile(s: WizardState): ChromeProfile | null {
  return s.profiles.find((p) => p.path === s.profile) ?? null;
}

/** Windows "Copy as path" wraps the path in quotes; people paste it like that. */
export function cleanPath(raw: string): string {
  const t = raw.trim();
  const m = t.match(/^"(.*)"$/) ?? t.match(/^'(.*)'$/);
  return (m ? m[1] : t).trim();
}

/** Why the wizard cannot start yet (empty = ready). */
export function problems(s: WizardState): string[] {
  const out: string[] = [];
  const any = s.parts.browsing || s.parts.passwords || s.parts.cookies;
  if (!any) out.push('Pick at least one thing to import.');
  if (s.parts.browsing) {
    const p = selectedProfile(s);
    if (!p) out.push('Pick a Chrome profile.');
    else if (!hasData(p)) out.push(`Profile “${p.name}” has no bookmarks or history.`);
  }
  if (s.parts.passwords && !cleanPath(s.passwordsCsv)) out.push('Enter the path of the exported passwords CSV.');
  if (s.parts.cookies && !cleanPath(s.cookiesJson)) out.push('Enter the path of the cookies JSON file.');
  return out;
}

export const canStart = (s: WizardState) => s.step === 'choose' && problems(s).length === 0;

/** The `import.chrome` params for the current choice, or null when it cannot start. */
export function importArgs(s: WizardState): ImportChromeArgs | null {
  if (problems(s).length) return null;
  const parts: ImportPart[] = [];
  const args: ImportChromeArgs = { parts };
  if (s.parts.browsing) {
    const p = selectedProfile(s)!;
    if (p.has_bookmarks) parts.push('bookmarks');
    if (p.has_history) parts.push('history');
    args.profile_dir = p.path;
  }
  if (s.parts.passwords) {
    parts.push('passwords_csv');
    args.passwords_csv_path = cleanPath(s.passwordsCsv);
  }
  if (s.parts.cookies) {
    parts.push('cookies_json');
    args.cookies_json_path = cleanPath(s.cookiesJson);
  }
  return args;
}

export function wizardReducer(s: WizardState, e: WizardEvent): WizardState {
  switch (e.type) {
    case 'detected': {
      if (s.step !== 'detecting') return s;
      const best = e.profiles.find(hasData) ?? null;
      return {
        ...s,
        step: 'choose',
        profiles: e.profiles,
        userDataDirs: e.userDataDirs,
        profile: best?.path ?? null,
        parts: { ...s.parts, browsing: !!best },
      };
    }
    case 'detect_failed':
      if (s.step !== 'detecting') return s;
      return { ...s, step: 'choose', detectError: e.error, parts: { ...s.parts, browsing: false } };
    case 'select_profile':
      if (s.step !== 'choose' || !s.profiles.some((p) => p.path === e.path)) return s;
      return { ...s, profile: e.path };
    case 'toggle':
      if (s.step !== 'choose') return s;
      // Nothing to import bookmarks/history from without a profile that has them.
      if (e.part === 'browsing' && e.on && !s.profiles.some(hasData)) return s;
      return { ...s, parts: { ...s.parts, [e.part]: e.on } };
    case 'set_path':
      if (s.step !== 'choose') return s;
      return e.part === 'passwords' ? { ...s, passwordsCsv: e.path } : { ...s, cookiesJson: e.path };
    case 'start':
      return canStart(s) ? { ...s, step: 'running', error: null, results: [] } : s;
    case 'finished':
      return s.step === 'running' ? { ...s, step: 'done', results: e.results } : s;
    case 'failed':
      return s.step === 'running' ? { ...s, step: 'choose', error: e.error } : s;
    case 'skip':
      return s.step === 'choose' || s.step === 'detecting' ? { ...s, step: 'skipped' } : s;
    case 'back':
      return s.step === 'done' || s.step === 'skipped' ? { ...s, step: 'choose', results: [] } : s;
  }
}

export const PART_LABELS: Record<ImportPart, string> = {
  bookmarks: 'Bookmarks',
  history: 'History',
  passwords_csv: 'Passwords',
  cookies_json: 'Cookies',
};

/** "12 imported, 3 skipped" for one result row. */
export function resultSummary(r: ImportPartResult): string {
  if (!r.ok) return r.error ?? 'failed';
  const skipped = r.skipped ? `, ${r.skipped} skipped` : '';
  return `${r.imported} imported${skipped}`;
}
