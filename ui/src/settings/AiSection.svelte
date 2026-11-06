<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import { AiStore } from '../lib/ai/store.svelte';
  import AiSettings from '../lib/components/panels/AiSettings.svelte';

  let { api }: { api: BrowserApi } = $props();

  // svelte-ignore state_referenced_locally
  const store = new AiStore(api);
  $effect(() => {
    void store.refreshProviders();
    return api.onStatus((s) => {
      if (s === 'open') void store.refreshProviders();
    });
  });

  const def = $derived(store.providers.find((p) => p.is_default) ?? null);
  const chat = $derived(store.providers.find((p) => p.id === store.provider) ?? null);
</script>

<div class="set-page">
  <header>
    <h2>AI</h2>
    <p>
      Providers for the AI side panel and for background features: self-healing of mass runs and
      <span class="set-mono">page.extract</span>. Keys are stored in <span class="set-mono">data/config/ai.json</span> on this
      computer; <b>Test</b> only lists models or runs <span class="set-mono">--version</span> and spends no tokens.
    </p>
  </header>

  <section class="set-card" aria-labelledby="ai-use">
    <h3 id="ai-use">In use</h3>
    <div class="set-row">
      <div class="text">
        <span class="label">New chats in the AI panel</span>
        <span class="help">Pick “Use for new chats” on a provider below.</span>
      </div>
      <div class="ctl"><span class="lib-chip">{chat?.name ?? store.provider}</span></div>
    </div>
    <div class="set-row">
      <div class="text">
        <span class="label">Background AI (self-healing, page.extract)</span>
        <span class="help">Without a default the first configured provider is used (Claude Code when installed).</span>
      </div>
      <div class="ctl"><span class="lib-chip" class:warn={!def}>{def?.name ?? 'Automatic'}</span></div>
    </div>
  </section>

  <section class="set-card providers" aria-labelledby="ai-prov">
    <h3 id="ai-prov">Providers</h3>
    <AiSettings {store} />
  </section>
</div>

<style>
  .providers :global(.settings) {
    padding-bottom: 6px;
  }
</style>
