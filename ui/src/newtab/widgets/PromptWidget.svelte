<script lang="ts">
  import { t } from '../../lib/i18n/index.svelte';
  import type { BrowserApi } from '../../lib/api';
  import Icon from '../../lib/components/Icon.svelte';
  import VoiceInput from '../../lib/components/voice/VoiceInput.svelte';
  import { openSettings } from '../../lib/settings/open';
  import type { AiProvider } from '../../lib/api';
  import { PROVIDERS, TASK_AGENTS } from '../../lib/providers';
  import { load, save } from '../../lib/storage';

  let { api }: { api: BrowserApi } = $props();

  // Agents that can take a task end-to-end (they open their own tab group). The live list
  // (`ai.providers`) says which are configured; until it arrives the static names are shown.
  let live = $state<AiProvider[] | null>(null);
  const AGENTS = $derived(
    TASK_AGENTS.map((id) => {
      const p = live?.find((x) => x.id === id);
      return { id, name: p?.name ?? PROVIDERS.find((x) => x.id === id)?.name ?? id, configured: p?.configured ?? true };
    }),
  );
  let provider = $state(load('newtab.provider', 'claude-code'));
  let text = $state('');
  let sending = $state(false);
  let status = $state<{ ok: boolean; text: string } | null>(null);
  let field = $state<HTMLTextAreaElement>();
  const name = $derived(AGENTS.find((p) => p.id === provider)?.name ?? 'AI');

  $effect(() => {
    api
      .call('ai.providers')
      .then((r) => (live = r.providers))
      .catch(() => {});
  });

  // Example tasks in the UI language (`newtab.prompt.example1..3`).
  const EXAMPLES = $derived([1, 2, 3].map((n) => t(`newtab.prompt.example${n}`)));

  async function submit() {
    const task = text.trim();
    if (!task || sending) return;
    sending = true;
    status = null;
    try {
      const session = await api.call('ai.session_start', { provider, prompt: task });
      status = {
        ok: true,
        text: t('newtab.prompt.started', { name, id: session.id }),
      };
      text = '';
    } catch (err) {
      status = { ok: false, text: t('newtab.prompt.rejected', { name, error: (err as Error).message }) };
    } finally {
      sending = false;
    }
  }

  function autosize() {
    if (!field) return;
    field.style.height = 'auto';
    field.style.height = `${Math.min(field.scrollHeight, 220)}px`;
  }

  /** Voice not set up yet: Settings → Voice opens in this tab (the new tab page has no chrome). */
  function openVoiceSettings() {
    openSettings({ tabs: { tabs: [] }, activate: () => {}, newTab: (url) => location.assign(url) }, 'voice');
  }
</script>

<section class="prompt" aria-labelledby="prompt-title">
  <h1 id="prompt-title">{t('newtab.prompt.title')}</h1>
  <form
    class="box"
    onsubmit={(e) => {
      e.preventDefault();
      void submit();
    }}
  >
    <!-- Voice dictation, bottom left of the box: hold Space in the empty box, or the mic button. -->
    <VoiceInput {api} target={field} onsubmit={() => void submit()} onsetup={openVoiceSettings} />
    <textarea
      bind:this={field}
      bind:value={text}
      oninput={autosize}
      onkeydown={(e) => {
        if (e.key === 'Enter' && !e.shiftKey) {
          e.preventDefault();
          void submit();
        }
      }}
      rows="1"
      placeholder={t('newtab.prompt.placeholder')}
      aria-labelledby="prompt-title"
    ></textarea>
    <div class="controls">
      <label class="select">
        <span class="sr-only">{t('newtab.prompt.agent')}</span>
        <select bind:value={provider} onchange={() => save('newtab.provider', provider)}>
          {#each AGENTS as p (p.id)}
            <option value={p.id}>{p.name}{p.configured ? '' : ` (${t('newtab.prompt.notSetUp')})`}</option>
          {/each}
        </select>
        <Icon name="chevron-down" size={12} stroke={2.25} />
      </label>
      <button class="btn primary" type="submit" disabled={!text.trim() || sending}>
        {sending ? t('newtab.prompt.sending') : t('newtab.prompt.start')}
        <kbd>Enter</kbd>
      </button>
    </div>
  </form>
  {#if status}
    <p class="status" class:err={!status.ok} role="status">{status.text}</p>
  {:else}
    <div class="examples">
      {#each EXAMPLES as ex (ex)}
        <button
          class="ex"
          onclick={() => {
            text = ex;
            field?.focus();
            requestAnimationFrame(autosize);
          }}>{ex}</button
        >
      {/each}
    </div>
  {/if}
</section>

<style>
  .prompt {
    display: flex;
    flex-direction: column;
    align-items: stretch;
  }
  h1 {
    margin: 0 0 16px;
    font-size: 30px;
    font-weight: 700;
    letter-spacing: -0.025em;
    line-height: 1.1;
    text-align: center;
    text-wrap: balance;
  }
  .box {
    display: flex;
    align-items: flex-end;
    gap: 10px;
    padding: 10px 10px 10px 18px;
    border: 1px solid var(--border-strong);
    border-radius: 16px;
    background: var(--surface);
    box-shadow: 0 6px 20px -10px rgb(14 17 22 / 0.3);
    transition: border-color 140ms, box-shadow 140ms;
  }
  /* With the mic button at the bottom left, the text starts right after it. */
  .box:has(:global(.voice)) {
    padding-left: 8px;
    gap: 6px;
  }
  .box:focus-within {
    border-color: var(--accent-line);
    box-shadow: 0 0 0 4px var(--accent-soft), 0 6px 20px -10px rgb(14 17 22 / 0.3);
  }
  textarea {
    flex: 1;
    min-height: 34px;
    max-height: 220px;
    padding: 7px 0;
    border: 0;
    outline: none;
    background: transparent;
    resize: none;
    font-size: var(--fs-lg);
    line-height: 1.4;
    user-select: text;
  }
  textarea::placeholder {
    color: var(--fg-subtle);
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
  .select {
    position: relative;
    display: inline-flex;
    align-items: center;
    color: var(--fg-muted);
  }
  .select :global(svg) {
    position: absolute;
    right: 8px;
    pointer-events: none;
  }
  select {
    appearance: none;
    height: 32px;
    padding: 0 26px 0 10px;
    border: 0;
    border-radius: var(--r);
    background: var(--sunken);
    color: var(--fg);
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .btn.primary {
    height: 32px;
    padding: 0 8px 0 13px;
  }
  .btn.primary kbd {
    border-color: transparent;
    background: color-mix(in oklab, var(--on-accent) 20%, transparent);
    color: inherit;
  }
  .examples {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: 6px;
    margin-top: 12px;
  }
  .ex {
    padding: 5px 11px;
    border: 1px solid var(--border);
    border-radius: 14px;
    background: transparent;
    color: var(--fg-muted);
    font-size: var(--fs-sm);
  }
  .ex:hover {
    border-color: var(--border-strong);
    color: var(--fg);
  }
  .status {
    margin: 12px 0 0;
    color: var(--ok);
    font-size: var(--fs-sm);
    text-align: center;
  }
  .status.err {
    color: var(--danger);
  }
  @media (max-width: 560px) {
    .box {
      flex-direction: column;
      align-items: stretch;
    }
    .controls {
      justify-content: space-between;
    }
  }
</style>
