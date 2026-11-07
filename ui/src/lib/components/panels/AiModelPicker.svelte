<script lang="ts">
  // The model picker of the AI panel: for the shown chat (ai.session_set_model, from the next
  // turn) or for the next new chat (ai.session_start's `model`). The list is `ai.models`.
  import { t } from '../../i18n/index.svelte';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';

  let {
    provider,
    value,
    title,
    allowDefault = true,
    onpick,
  }: {
    provider: string;
    /** The model in effect; "" = the provider's default. */
    value: string;
    title: string;
    /** Offer "Default" (the new-chat page); a chat's model is always a real one. */
    allowDefault?: boolean;
    onpick: (model: string) => void;
  } = $props();

  const ai = useChrome().ai;

  $effect(() => {
    void ai.loadModels(provider);
  });

  const list = $derived(ai.models[provider]);
  const error = $derived(ai.modelErrors[provider]);
  const providerModel = $derived(ai.providers.find((p) => p.id === provider)?.model ?? null);
  const options = $derived.by(() => {
    const models = (list?.models ?? []).map((m) => ({ id: m.id, label: m.hint ? `${m.name} — ${m.hint}` : m.name }));
    // The chat's model may be one the list does not offer (set over the API, an older id).
    if (value && !models.some((m) => m.id === value)) models.unshift({ id: value, label: value });
    return models;
  });
  const defaultLabel = $derived(
    providerModel ? t('aiChat.model.defaultNamed', { name: providerModel }) : t('aiChat.model.default'),
  );
  const tooltip = $derived(error ? t('aiChat.model.failed', { message: error }) : title);
</script>

<label class="model" title={tooltip}>
  <span class="sr-only">{t('aiChat.model.label')}</span>
  <Icon name="sparkles" size={11} />
  <select
    {value}
    onchange={(e) => {
      const next = e.currentTarget.value;
      if (next !== value) onpick(next);
    }}
  >
    {#if allowDefault || !value}
      <option value="" disabled={!allowDefault}>{defaultLabel}</option>
    {/if}
    {#each options as m (m.id)}
      <option value={m.id}>{m.label}</option>
    {/each}
    {#if !list && !error}
      <option disabled value="__loading">{t('aiChat.model.loading')}</option>
    {:else if list && list.models.length === 0}
      <option disabled value="__none">{t('aiChat.model.none')}</option>
    {/if}
  </select>
  <Icon name="chevron-down" size={11} stroke={2.25} />
</label>

<style>
  .model {
    position: relative;
    display: inline-flex;
    align-items: center;
    min-width: 0;
    height: 22px;
    padding: 0 18px 0 6px;
    gap: 4px;
    border-radius: 11px;
    background: var(--sunken);
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .model:hover {
    color: var(--fg);
    background: var(--hover);
  }
  .model:focus-within {
    box-shadow: 0 0 0 2px var(--accent-line);
  }
  .model :global(svg:last-child) {
    position: absolute;
    right: 6px;
    pointer-events: none;
  }
  select {
    appearance: none;
    min-width: 0;
    max-width: 150px;
    height: 100%;
    padding: 0;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    text-overflow: ellipsis;
    outline: none;
  }
  option {
    color: var(--fg);
    background: var(--surface);
    font-weight: 400;
  }
</style>
