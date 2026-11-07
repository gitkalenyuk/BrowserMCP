<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import { AiStore } from '../lib/ai/store.svelte';
  import AiSettings from '../lib/components/panels/AiSettings.svelte';
  import { t } from '../lib/i18n/index.svelte';

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

  /** "Text with `code`" → parts, odd ones shown as code (translations keep the backticks). */
  function codeParts(text: string): { text: string; code: boolean }[] {
    return text.split('`').map((part, i) => ({ text: part, code: i % 2 === 1 }));
  }
</script>

<div class="set-page">
  <header>
    <h2>{t('aiProviders.section.title')}</h2>
    <p>
      {#each codeParts(t('aiProviders.section.lead')) as part, i (i)}
        {#if part.code}<span class="set-mono">{part.text}</span>{:else}{part.text}{/if}
      {/each}
    </p>
  </header>

  <section class="set-card" aria-labelledby="ai-use">
    <h3 id="ai-use">{t('aiProviders.section.inUse')}</h3>
    <div class="set-row">
      <div class="text">
        <span class="label">{t('aiProviders.section.chat')}</span>
        <span class="help">{t('aiProviders.section.chatHelp')}</span>
      </div>
      <div class="ctl"><span class="lib-chip">{chat?.name ?? store.provider}</span></div>
    </div>
    <div class="set-row">
      <div class="text">
        <span class="label">{t('aiProviders.section.background')}</span>
        <span class="help">{t('aiProviders.section.backgroundHelp')}</span>
      </div>
      <div class="ctl"><span class="lib-chip" class:warn={!def}>{def?.name ?? t('aiProviders.section.automatic')}</span></div>
    </div>
  </section>

  <section class="set-card providers" aria-labelledby="ai-prov">
    <h3 id="ai-prov">{t('aiProviders.section.providers')}</h3>
    <AiSettings {store} />
  </section>
</div>

<style>
  .providers :global(.settings) {
    padding-bottom: 6px;
  }
</style>
