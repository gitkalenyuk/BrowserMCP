<script lang="ts">
  import type { TabInfo, Workspace } from '../api/types';
  import { COMMANDS, type CommandMeta } from '../commands';
  import { fuzzyFilter, highlight, type FuzzyMatch } from '../fuzzy';
  import { chordLabels } from '../keymap';
  import { useChrome } from '../state/chrome.svelte';
  import { fallbackTitle, hostOf } from '../urls';
  import Favicon from './Favicon.svelte';
  import Icon from './Icon.svelte';

  type Item =
    | { kind: 'command'; key: string; text: string; cmd: CommandMeta }
    | { kind: 'tab'; key: string; text: string; tab: TabInfo }
    | { kind: 'workspace'; key: string; text: string; ws: Workspace };

  /** `standalone`: the page is only the palette (the browser's Ctrl+K overlay view). */
  let { standalone = false }: { standalone?: boolean } = $props();

  const chrome = useChrome();
  let query = $state('');
  let selected = $state(0);
  let input = $state<HTMLInputElement>();
  let list = $state<HTMLDivElement>();
  let restoreFocus: HTMLElement | null = null;

  const items = $derived.by<Item[]>(() => [
    ...chrome.tabs.tabs.map((tab) => ({
      kind: 'tab' as const,
      key: `tab-${tab.id}`,
      text: tab.title || fallbackTitle(tab.url),
      tab,
    })),
    ...COMMANDS.filter((c) => !c.hidden).map((cmd) => ({
      kind: 'command' as const,
      key: `cmd-${cmd.id}`,
      text: cmd.title,
      cmd,
    })),
    ...chrome.workspaces
      .filter((w) => w.id !== chrome.workspaceId)
      .map((ws) => ({ kind: 'workspace' as const, key: `ws-${ws.id}`, text: `Switch to workspace ${ws.name}`, ws })),
  ]);

  const results = $derived.by<{ item: Item; match: FuzzyMatch }[]>(() => {
    if (!query.trim()) {
      // No query: commands first, then tabs (Chrome-style tab search via typing).
      const order = { command: 0, tab: 1, workspace: 2 };
      return [...items]
        .sort((a, b) => order[a.kind] - order[b.kind])
        .map((item) => ({ item, match: { score: 0, indices: [] } }));
    }
    // Tabs also match on their URL host.
    return fuzzyFilter(query, items, (i) => (i.kind === 'tab' ? `${i.text} ${hostOf(i.tab.url)}` : i.text)).slice(0, 50);
  });

  $effect(() => {
    if (chrome.paletteOpen) {
      restoreFocus = document.activeElement as HTMLElement | null;
      query = '';
      selected = 0;
      // Focus now so keystrokes right after Ctrl+K land in the field; retry next frame
      // in case the element was not bound yet.
      window.focus(); // pull focus back from an embedded frame (dev new tab page)
      input?.focus();
      requestAnimationFrame(() => input?.focus());
    }
  });

  $effect(() => {
    void query;
    selected = 0;
  });

  $effect(() => {
    const el = list?.querySelector<HTMLElement>(`[data-index="${selected}"]`);
    el?.scrollIntoView({ block: 'nearest' });
  });

  function close() {
    chrome.paletteOpen = false;
    restoreFocus?.focus?.();
  }

  function exec(item: Item | undefined) {
    if (!item) return;
    close();
    if (item.kind === 'command') chrome.run(item.cmd.id);
    else if (item.kind === 'tab') chrome.activate(item.tab.id);
    else chrome.switchWorkspace(item.ws.id);
  }

  function onKey(e: KeyboardEvent) {
    const n = results.length;
    if (e.key === 'ArrowDown' || (e.key === 'Tab' && !e.shiftKey) || (e.ctrlKey && e.key === 'n')) {
      e.preventDefault();
      if (n) selected = (selected + 1) % n;
    } else if (e.key === 'ArrowUp' || (e.key === 'Tab' && e.shiftKey) || (e.ctrlKey && e.key === 'p')) {
      e.preventDefault();
      if (n) selected = (selected - 1 + n) % n;
    } else if (e.key === 'PageDown') {
      e.preventDefault();
      selected = Math.min(n - 1, selected + 8);
    } else if (e.key === 'PageUp') {
      e.preventDefault();
      selected = Math.max(0, selected - 8);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      exec(results[selected]?.item);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close();
    }
  }

  const KIND_LABEL = { tab: 'Tab', command: 'Command', workspace: 'Workspace' } as const;
</script>

{#if chrome.paletteOpen}
  {#if !standalone}
    <div class="scrim" role="presentation" onpointerdown={close}></div>
  {/if}
  <div
    class="palette"
    class:standalone
    role="dialog"
    aria-modal="true"
    aria-label="Command palette"
  >
    <div class="search">
      <Icon name="search" size={16} stroke={2} />
      <input
        bind:this={input}
        bind:value={query}
        onkeydown={onKey}
        placeholder="Search commands and open tabs"
        aria-label="Search commands and tabs"
        role="combobox"
        aria-expanded="true"
        aria-controls="palette-list"
        aria-activedescendant={results.length ? `palette-opt-${selected}` : undefined}
        spellcheck="false"
        autocomplete="off"
      />
      <kbd>Esc</kbd>
    </div>
    <div class="list" id="palette-list" role="listbox" bind:this={list}>
      {#each results as { item, match }, i (item.key)}
        <div
          class="row"
          class:sel={i === selected}
          id="palette-opt-{i}"
          role="option"
          tabindex="-1"
          aria-selected={i === selected}
          data-index={i}
          onpointermove={() => (selected = i)}
          onclick={() => exec(item)}
          onkeydown={() => {}}
        >
          <span class="lead">
            {#if item.kind === 'tab'}
              <Favicon url={item.tab.url} src={item.tab.favicon} loading={item.tab.loading} />
            {:else if item.kind === 'workspace'}
              <span class="ws-dot" style:--c="var(--g-{item.ws.color})"></span>
            {:else}
              <Icon name="chevron-right" size={14} stroke={2} />
            {/if}
          </span>
          <span class="text">
            {#each highlight(item.text, match.indices.filter((x) => x < item.text.length)) as part, j (j)}
              {#if part.hit}<mark>{part.text}</mark>{:else}{part.text}{/if}
            {/each}
          </span>
          {#if item.kind === 'tab'}
            <span class="sub">{hostOf(item.tab.url)}</span>
          {:else if item.kind === 'workspace'}
            <span class="sub">{item.ws.profile} · {item.ws.proxy}</span>
          {/if}
          {#if item.kind === 'command' && chrome.keymap[item.cmd.id]?.[0]}
            <span class="keys">
              {#each chordLabels(chrome.keymap[item.cmd.id][0]) as k (k)}<kbd>{k}</kbd>{/each}
            </span>
          {:else}
            <span class="kind">{KIND_LABEL[item.kind]}</span>
          {/if}
        </div>
      {:else}
        <div class="none">No commands or tabs match “{query}”.</div>
      {/each}
    </div>
    <div class="foot">
      <span><kbd>↑</kbd><kbd>↓</kbd> to move</span>
      <span><kbd>Enter</kbd> to run</span>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 40;
    background: var(--scrim);
    animation: fade 120ms ease-out;
  }
  .palette {
    position: fixed;
    z-index: 41;
    top: 56px;
    left: 50%;
    display: flex;
    flex-direction: column;
    width: min(620px, calc(100vw - 32px));
    max-height: min(520px, calc(100vh - 80px));
    border: 1px solid var(--border);
    border-radius: 12px;
    background: var(--surface);
    box-shadow: var(--shadow-pop);
    transform: translateX(-50%);
    animation: drop 160ms var(--ease-out);
    overflow: hidden;
  }
  /* The overlay view is exactly the palette's size: fill it, no shadow or animation
     (a native view cannot be transparent around rounded corners). */
  .palette.standalone {
    inset: 0;
    top: 0;
    left: 0;
    width: 100%;
    max-height: none;
    border-radius: 0;
    box-shadow: none;
    transform: none;
    animation: none;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 48px;
    padding: 0 14px;
    border-bottom: 1px solid var(--border);
    color: var(--fg-subtle);
    flex: none;
  }
  input {
    flex: 1;
    height: 100%;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--fg);
    font-size: var(--fs-lg);
    user-select: text;
  }
  input::placeholder {
    color: var(--fg-subtle);
  }
  .list {
    flex: 1;
    min-height: 0;
    padding: 6px;
    overflow-y: auto;
    scrollbar-width: thin;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 34px;
    padding: 0 10px;
    border-radius: var(--r);
    color: var(--fg);
  }
  .row.sel {
    background: var(--accent-soft);
  }
  .lead {
    display: inline-grid;
    place-items: center;
    width: 18px;
    color: var(--fg-subtle);
    flex: none;
  }
  .text {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  mark {
    background: transparent;
    color: var(--accent);
    font-weight: 650;
  }
  .sub {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
  }
  .keys,
  .kind {
    margin-left: auto;
    flex: none;
  }
  .keys {
    display: inline-flex;
    gap: 3px;
  }
  .kind {
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
  .ws-dot {
    width: 9px;
    height: 9px;
    border-radius: 3px;
    background: var(--c);
  }
  .none {
    padding: 22px 12px;
    color: var(--fg-muted);
    text-align: center;
  }
  .foot {
    display: flex;
    gap: 16px;
    padding: 7px 14px;
    border-top: 1px solid var(--border);
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    flex: none;
  }
  .foot kbd {
    margin-right: 3px;
  }
  @keyframes drop {
    from {
      opacity: 0;
      transform: translate(-50%, -6px) scale(0.985);
    }
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
</style>
