<script lang="ts">
  import CommandPalette from '../lib/components/CommandPalette.svelte';
  import { applyTheme, settings } from '../lib/settings.svelte';
  import { provideChrome, type ChromeController } from '../lib/state/chrome.svelte';

  let { chrome }: { chrome: ChromeController } = $props();
  // svelte-ignore state_referenced_locally
  provideChrome(chrome);

  $effect(() => applyTheme(document.documentElement, settings.value));

  // The palette closed itself (Esc in the field, a command ran, a click): hide the
  // overlay view too. (When the browser hides it, it tells us with onHide first.)
  let wasOpen = false;
  $effect(() => {
    const open = chrome.paletteOpen;
    if (wasOpen && !open) chrome.host?.palette('hide');
    wasOpen = open;
  });
</script>

<CommandPalette standalone={!!chrome.host} />

<style>
  :global(body) {
    background: var(--surface);
    overflow: hidden;
  }
</style>
