// The English guide in reading order (ids: model.ts CHAPTER_IDS).
import type { Chapter } from '../../model';
import { aiPanel, connect, perception } from './ai';
import { recording, runs, scripts, traffic } from './automation';
import { keyboard, library, newtab, start, tabs } from './basics';
import { glossary, troubleshooting } from './help';
import { adblock, data, languages, notifications, performance, stealth } from './privacy';
import { voice } from './voice';

const chapters: Chapter[] = [
  start,
  tabs,
  keyboard,
  library,
  newtab,
  aiPanel,
  voice,
  connect,
  perception,
  traffic,
  recording,
  scripts,
  runs,
  stealth,
  adblock,
  performance,
  notifications,
  languages,
  data,
  troubleshooting,
  glossary,
];
export default chapters;
