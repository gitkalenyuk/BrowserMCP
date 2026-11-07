// Where the guide opens: `#guide/<chapter>[/<section>]` anchors and the chapter that explains
// each settings section (the "Help" link in the settings header).

import { parseGuideHash, type SettingsSection } from '../../lib/settings/open';
import type { Chapter } from './model';

export interface GuideAnchor {
  chapter: string;
  section?: string;
}

/** "tabs/split" → {chapter: "tabs", section: "split"}; null for anything else. */
export function parseAnchor(path: string | null): GuideAnchor | null {
  if (!path) return null;
  const valid = parseGuideHash(`guide/${path}`);
  if (!valid) return null;
  const [chapter, section] = valid.split('/');
  return section ? { chapter, section } : { chapter };
}

export function anchorPath(a: GuideAnchor): string {
  return a.section ? `${a.chapter}/${a.section}` : a.chapter;
}

/** An anchor that exists in `chapters` (an unknown section falls back to its chapter). */
export function resolveAnchor(chapters: Chapter[], a: GuideAnchor | null): GuideAnchor | null {
  const ch = a && chapters.find((c) => c.id === a.chapter);
  if (!ch) return null;
  return a.section && ch.sections.some((s) => s.id === a.section) ? { chapter: ch.id, section: a.section } : { chapter: ch.id };
}

/** The guide chapter for each settings section (guide.test.ts checks that none is missing). */
export const HELP_FOR: Record<Exclude<SettingsSection, 'guide'>, GuideAnchor> = {
  general: { chapter: 'newtab' },
  ai: { chapter: 'ai-panel' },
  notifications: { chapter: 'notifications' },
  performance: { chapter: 'performance' },
  privacy: { chapter: 'adblock' },
  hints: { chapter: 'keyboard', section: 'hints' },
  plugins: { chapter: 'scripts', section: 'plugins' },
  keyboard: { chapter: 'keyboard', section: 'shortcuts' },
  startup: { chapter: 'tabs', section: 'restore' },
  downloads: { chapter: 'library', section: 'downloads' },
  search: { chapter: 'keyboard', section: 'omnibox' },
  sites: { chapter: 'data', section: 'permissions' },
  data: { chapter: 'data' },
  about: { chapter: 'start' },
  connections: { chapter: 'connect' },
  profiles: { chapter: 'stealth', section: 'identity' },
  proxies: { chapter: 'stealth', section: 'proxies' },
  recording: { chapter: 'recording' },
};

export function helpFor(section: string): GuideAnchor | null {
  return (HELP_FOR as Record<string, GuideAnchor>)[section] ?? null;
}
