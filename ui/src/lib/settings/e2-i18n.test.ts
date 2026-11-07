// Every key the four E2 settings sections and their helpers use exists in the English
// dictionary (the i18n test then checks Ukrainian against English). Keys built at run time
// (`settings.x.${name}`) are listed here from the values the code can produce.
import { describe, expect, it } from 'vitest';
import { DICTS } from '../i18n/index.svelte';

const sources = import.meta.glob<string>(
  [
    '../../settings/ConnectionsSection.svelte',
    '../../settings/ProfilesSection.svelte',
    '../../settings/ProxiesSection.svelte',
    '../../settings/RecordingSection.svelte',
    './e2-*.ts',
    '!./e2-*.test.ts',
  ],
  { query: '?raw', import: 'default', eager: true },
);

const PLURAL = ['one', 'few', 'many', 'other'];

const dynamic = [
  ...['core', 'net', 'record', 'script', 'run', 'profile', 'stealth', 'data', 'ai', 'ui', 'system'].map((n) => `settings.connections.toolset.${n}`),
  ...['idle', 'running', 'waiting', 'error'].map((s) => `settings.connections.agent.${s}`),
  ...['env', 'next_to_exe', 'path'].map((s) => `settings.recording.ffmpeg.source.${s}`),
  ...['recordings', 'videos', 'scripts', 'runs'].flatMap((f) => [`settings.recording.folders.${f}`, `settings.recording.folders.${f}Help`]),
  ...['windows', 'macos', 'android', 'ios'].map((os) => `settings.profiles.os.${os}`),
  ...['desktop', 'mobile', 'emulation'].map((g) => `settings.profiles.preset.${g}`),
  ...['round_robin', 'random', 'sticky'].flatMap((s) => [`settings.proxies.strategy.${s}`, `settings.proxies.strategy.${s}Help`]),
];

describe('E2 settings strings', () => {
  it('finds the sources', () => {
    expect(Object.keys(sources).length).toBeGreaterThanOrEqual(8);
  });

  it('has every key the sections use', () => {
    const missing: string[] = [];
    for (const [file, text] of Object.entries(sources)) {
      for (const m of text.matchAll(/\b(tp?)\(\s*'(settings\.[\w.]+)'/g)) {
        const keys = m[1] === 'tp' ? PLURAL.map((f) => `${m[2]}.${f}`) : [m[2]];
        for (const k of keys) if (!(k in DICTS.en)) missing.push(`${file}: ${k}`);
      }
      // Helpers return keys as plain strings for the section to translate.
      if (file.endsWith('.ts')) {
        for (const m of text.matchAll(/'(settings\.(?:connections|profiles|proxies|recording)\.[\w.]+)'/g)) {
          if (!(m[1] in DICTS.en)) missing.push(`${file}: ${m[1]}`);
        }
      }
    }
    for (const k of dynamic) if (!(k in DICTS.en)) missing.push(`dynamic: ${k}`);
    expect(missing).toEqual([]);
  });
});
