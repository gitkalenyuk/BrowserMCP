<script lang="ts">
  import { useChrome } from '../state/chrome.svelte';
  import { displayUrl, isNewTab, omniboxIntent } from '../urls';
  import Icon, { type IconName } from './Icon.svelte';

  const chrome = useChrome();
  let input: HTMLInputElement;
  let focused = $state(false);
  let draft = $state('');
  const url = $derived(chrome.active?.url ?? '');
  const value = $derived(focused ? draft : displayUrl(url));

  // What Enter would do with the current draft, shown as a hint while editing.
  const intent = $derived.by(() => {
    if (!focused || draft === displayUrl(url)) return null;
    return omniboxIntent(draft);
  });

  const icon: IconName = $derived.by(() => {
    if (intent === 'search') return 'search';
    if (focused || isNewTab(url)) return 'search';
    if (url.startsWith('https://')) return 'lock';
    return 'info';
  });
  const iconLabel = $derived(
    icon === 'lock' ? 'Connection is secure' : icon === 'info' ? 'Connection is not secure' : 'Search or type URL',
  );

  // Let commands (Ctrl+L, new tab) focus the field.
  $effect(() => {
    if (chrome.omniboxFocusTick === 0) return;
    window.focus();
    input?.focus();
    input?.select();
  });

  function onFocus() {
    const initial = displayUrl(url);
    draft = initial;
    focused = true;
    input.select();
    // Select again once focus has settled (a click's mouseup would collapse the selection),
    // unless the user already started typing.
    requestAnimationFrame(() => input.value === initial && input.select());
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      if (chrome.navigate(draft)) input.blur();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      if (draft !== displayUrl(url)) {
        draft = displayUrl(url);
        requestAnimationFrame(() => input.select());
      } else {
        input.blur();
      }
    }
  }
</script>

<div class="omnibox" class:focused>
  <span class="icon" class:secure={icon === 'lock'} title={iconLabel}>
    <Icon name={icon} size={14} stroke={2} />
  </span>
  <input
    bind:this={input}
    {value}
    oninput={(e) => (draft = e.currentTarget.value)}
    onfocus={onFocus}
    onblur={() => (focused = false)}
    onkeydown={onKey}
    type="text"
    spellcheck="false"
    autocomplete="off"
    autocapitalize="off"
    placeholder="Search or enter address"
    aria-label="Address and search bar"
  />
  {#if intent}
    <span class="hint" aria-hidden="true">
      {intent === 'search' ? 'Search Google' : 'Open'} <kbd>Enter</kbd>
    </span>
  {/if}
</div>

<style>
  .omnibox {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: 1;
    min-width: 120px;
    height: 30px;
    padding: 0 6px 0 9px;
    border-radius: 15px;
    background: var(--field);
    border: 1px solid transparent;
    transition: background-color 120ms, border-color 120ms, box-shadow 120ms;
  }
  .omnibox:hover:not(.focused) {
    background: color-mix(in oklab, var(--field) 85%, var(--fg) 6%);
  }
  .omnibox.focused {
    background: var(--field-focus);
    border-color: var(--accent-line);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .icon {
    display: inline-grid;
    place-items: center;
    width: 18px;
    color: var(--fg-subtle);
    flex: none;
  }
  .icon.secure {
    color: var(--fg-muted);
  }
  input {
    flex: 1;
    min-width: 0;
    height: 100%;
    padding: 0;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--fg);
    font-size: var(--fs);
    user-select: text;
  }
  input::placeholder {
    color: var(--fg-subtle);
  }
  .hint {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    white-space: nowrap;
  }
</style>
