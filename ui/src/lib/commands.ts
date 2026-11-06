import type { CommandId } from './keymap';

export interface CommandMeta {
  id: CommandId;
  title: string;
  /** Not listed in the palette (still bound to keys). */
  hidden?: boolean;
}

// Static palette entries. Later this list comes from the core's Command Registry
// (Meta.palette), and execution is routed through it.
export const COMMANDS: CommandMeta[] = [
  { id: 'tab.new', title: 'New tab' },
  { id: 'tab.close', title: 'Close tab' },
  { id: 'tab.reopen', title: 'Reopen closed tab' },
  { id: 'tab.next', title: 'Next tab' },
  { id: 'tab.prev', title: 'Previous tab' },
  { id: 'nav.back', title: 'Go back' },
  { id: 'nav.forward', title: 'Go forward' },
  { id: 'nav.reload', title: 'Reload page' },
  { id: 'nav.stop', title: 'Stop loading' },
  { id: 'omnibox.focus', title: 'Focus address bar' },
  { id: 'ui.toggleVerticalTabs', title: 'Toggle vertical tabs' },
  { id: 'ui.toggleTheme', title: 'Toggle light / dark theme' },
  { id: 'panel.traffic', title: 'Open Traffic panel' },
  { id: 'panel.recording', title: 'Open Recording panel' },
  { id: 'panel.ai', title: 'Open AI panel' },
  { id: 'panel.terminal', title: 'Open Terminal panel' },
  { id: 'panel.scripts', title: 'Open Scripts panel' },
  { id: 'panel.runs', title: 'Open Runs panel' },
  { id: 'panel.close', title: 'Close side panel' },
  { id: 'library.bookmarks', title: 'Bookmarks manager' },
  { id: 'library.history', title: 'History' },
  { id: 'library.downloads', title: 'Downloads' },
  { id: 'library.passwords', title: 'Passwords' },
  { id: 'library.import', title: 'Import bookmarks, history and passwords from Chrome' },
  { id: 'bookmark.toggle', title: 'Bookmark this page / remove bookmark' },
  { id: 'ui.toggleBookmarksBar', title: 'Show / hide bookmarks bar' },
  { id: 'palette.open', title: 'Command palette', hidden: true },
  ...([1, 2, 3, 4, 5, 6, 7, 8] as const).map((n) => ({
    id: `tab.select${n}` as CommandId,
    title: `Select tab ${n}`,
    hidden: true,
  })),
  { id: 'tab.selectLast', title: 'Select last tab', hidden: true },
];
