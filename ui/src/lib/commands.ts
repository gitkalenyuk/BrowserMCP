import { t } from './i18n/index.svelte';
import type { CommandId } from './keymap';

export interface CommandMeta {
  id: CommandId;
  /** In the current UI language (a getter: read it in markup or `$derived`). */
  readonly title: string;
  /** Not listed in the palette (still bound to keys). */
  hidden?: boolean;
}

/** The command's title in the current UI language (`commands.<id>` in lib/i18n). */
export function commandTitle(id: CommandId): string {
  return t(`commands.${id}`);
}

// The title is a getter so the list can stay a constant and still follow the language.
function cmd(id: CommandId, hidden?: boolean): CommandMeta {
  return {
    id,
    get title() {
      return commandTitle(id);
    },
    ...(hidden ? { hidden } : {}),
  };
}

// Static palette entries. Later this list comes from the core's Command Registry
// (Meta.palette), and execution is routed through it.
export const COMMANDS: CommandMeta[] = [
  cmd('tab.new'),
  cmd('tab.close'),
  cmd('tab.reopen'),
  cmd('tab.next'),
  cmd('tab.prev'),
  cmd('nav.back'),
  cmd('nav.forward'),
  cmd('nav.reload'),
  cmd('nav.stop'),
  cmd('omnibox.focus'),
  // Hidden: in the browser the palette is another page, which has no find bar to open.
  cmd('find.open', true),
  cmd('hints.show'),
  cmd('ui.toggleVerticalTabs'),
  cmd('ui.toggleTheme'),
  cmd('panel.traffic'),
  cmd('panel.recording'),
  cmd('panel.ai'),
  cmd('panel.terminal'),
  cmd('panel.scripts'),
  cmd('panel.runs'),
  cmd('panel.close'),
  cmd('library.bookmarks'),
  cmd('library.history'),
  cmd('library.downloads'),
  cmd('library.passwords'),
  cmd('library.import'),
  cmd('bookmark.toggle'),
  cmd('ui.toggleBookmarksBar'),
  cmd('tab.split'),
  cmd('split.focusLeft', true),
  cmd('split.focusRight', true),
  cmd('zoom.in'),
  cmd('zoom.out'),
  cmd('zoom.reset'),
  cmd('settings.open'),
  cmd('help.guide'),
  cmd('palette.open', true),
  ...([1, 2, 3, 4, 5, 6, 7, 8] as const).map((n) => cmd(`tab.select${n}`, true)),
  cmd('tab.selectLast', true),
];
