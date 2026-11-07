<script lang="ts">
  // Test-only wrapper (AiPanel.test.ts): provides the chrome context the AI panel reads.
  import { provideChrome, type ChromeController } from '../../state/chrome.svelte';
  import AiPanel from './AiPanel.svelte';

  let { chrome }: { chrome: ChromeController } = $props();
  // svelte-ignore state_referenced_locally
  provideChrome(chrome);
</script>

<AiPanel />
