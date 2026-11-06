<script lang="ts">
  // Ctrl+F find bar under the toolbar. The page highlights and scrolling are done by the
  // core (page.find_text / page.find_clear); this only keeps the query and the counter.
  import { tick } from 'svelte';
  import { findArgs, findLabel, type FindStep } from '../find';
  import { useChrome } from '../state/chrome.svelte';
  import Icon from './Icon.svelte';

  const chrome = useChrome();
  let open = $state(false);
  let query = $state('');
  let count = $state(0);
  let index = $state(0);
  let unavailable = $state(false);
  let input: HTMLInputElement | undefined = $state();
  /** Tab whose page currently shows our highlights. */
  let searched: number | null = null;
  let debounce: ReturnType<typeof setTimeout> | undefined;

  const label = $derived(unavailable ? 'Not available' : findLabel(query, count, index));

  // Ctrl+F (chrome.findTick): open, focus and select the query.
  $effect(() => {
    if (chrome.findTick === 0) return;
    open = true;
    void tick().then(() => {
      input?.focus();
      input?.select();
    });
  });

  // Switching tabs while open: search the new tab, clear the old one.
  $effect(() => {
    const tab = chrome.tabs.activeId;
    if (!open || tab === searched || !query) return;
    clearHighlights();
    void search(0);
  });

  async function search(step: FindStep): Promise<void> {
    const tab = chrome.tabs.activeId;
    if (tab === null) return;
    if (!query) return clearHighlights();
    try {
      const r = await chrome.api.call('page.find_text', findArgs(tab, query, step));
      if (tab !== chrome.tabs.activeId) return; // the user switched tabs meanwhile
      searched = tab;
      count = r.count;
      index = r.index;
      unavailable = false;
    } catch {
      // The dev mock has no pages; a real page may also refuse (e.g. a dialog is open).
      count = 0;
      index = 0;
      unavailable = true;
    }
  }

  function clearHighlights(): void {
    if (searched !== null) void chrome.api.call('page.find_clear', { tab: searched }).catch(() => {});
    searched = null;
    count = 0;
    index = 0;
  }

  function onInput(): void {
    clearTimeout(debounce);
    debounce = setTimeout(() => void search(0), 150);
  }

  function close(): void {
    clearTimeout(debounce);
    clearHighlights();
    open = false;
    query = '';
    unavailable = false;
    chrome.host?.focusContent();
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.key === 'Escape') {
      e.preventDefault();
      close();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      clearTimeout(debounce);
      void search(e.shiftKey ? -1 : 1);
    }
  }
</script>

{#if open}
  <div class="find" role="search">
    <Icon name="search" size={14} />
    <input
      bind:this={input}
      bind:value={query}
      oninput={onInput}
      onkeydown={onKeydown}
      placeholder="Find in page"
      aria-label="Find in page"
      spellcheck="false"
    />
    <span class="count" aria-live="polite">{label}</span>
    <button class="icon-btn" aria-label="Previous match" disabled={count === 0} onclick={() => search(-1)}>
      <Icon name="chevron-down" size={14} class="up" />
    </button>
    <button class="icon-btn" aria-label="Next match" disabled={count === 0} onclick={() => search(1)}>
      <Icon name="chevron-down" size={14} />
    </button>
    <button class="icon-btn" aria-label="Close find bar" onclick={close}><Icon name="x" size={14} /></button>
  </div>
{/if}

<style>
  .find {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 6px;
    height: 34px;
    padding: 0 6px 0 14px;
    background: var(--toolbar);
    border-bottom: 1px solid var(--border);
    color: var(--fg-muted);
    font-size: var(--fs-sm);
  }
  input {
    width: min(280px, 50vw);
    height: 24px;
    padding: 0 8px;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--field);
    color: var(--fg);
    font: inherit;
  }
  input:focus {
    outline: none;
    border-color: var(--accent-line);
    background: var(--field-focus);
  }
  .count {
    min-width: 72px;
    font-variant-numeric: tabular-nums;
    color: var(--fg-subtle);
  }
  :global(.find svg.up) {
    transform: rotate(180deg);
  }
</style>
