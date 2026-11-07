<script lang="ts">
  import { t } from '../../lib/i18n/index.svelte';
  import type { AiModels, AiProvider, AiSession, BrowserApi } from '../../lib/api';
  import Icon from '../../lib/components/Icon.svelte';
  import { openAiPanel } from '../../lib/native';
  import { PROVIDERS, TASK_AGENTS } from '../../lib/providers';
  import { load, save } from '../../lib/storage';
  import { chatTitle, defaultModelName, pickModel, rememberModel, type ModelChoices } from '../prompt';

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
  /** Models the selected provider offers (`ai.models`); null = unknown (hide the picker). */
  let offered = $state<AiModels | null>(null);
  let choices = $state<ModelChoices>(load('newtab.models', {}));
  /** '' = the provider's default model. */
  let model = $state('');
  let text = $state('');
  let sending = $state(false);
  let status = $state<{ ok: true; text: string; session: string } | { ok: false; text: string } | null>(null);
  let field: HTMLTextAreaElement;
  const name = $derived(AGENTS.find((p) => p.id === provider)?.name ?? 'AI');
  const defaultName = $derived(defaultModelName(offered));
  const modelHint = $derived(offered?.models.find((m) => m.id === model)?.hint ?? t('newtab.prompt.model'));

  $effect(() => {
    api
      .call('ai.providers')
      .then((r) => (live = r.providers))
      .catch(() => {});
  });

  // The model list of the chosen provider; the remembered choice is preselected.
  $effect(() => {
    const id = provider;
    let current = true;
    offered = null;
    model = '';
    api
      .call('ai.models', { provider: id })
      .then((r) => {
        if (!current) return;
        offered = r.models.length ? r : null;
        model = pickModel(choices, id, offered);
      })
      .catch(() => {
        // An older core without ai.models, or a provider that cannot list models: no picker.
      });
    return () => (current = false);
  });

  function chooseModel(id: string) {
    model = id;
    choices = rememberModel(choices, provider, id);
    save('newtab.models', choices);
  }

  // Example tasks in the UI language (`newtab.prompt.example1..3`).
  const EXAMPLES = $derived([1, 2, 3].map((n) => t(`newtab.prompt.example${n}`)));

  async function submit() {
    const task = text.trim();
    if (!task || sending) return;
    sending = true;
    status = null;
    try {
      const session = await api.call('ai.session_start', { provider, prompt: task, ...(model ? { model } : {}) });
      // Another stream gives sessions a title; until then the task's first line names the chat.
      const title = chatTitle(task, (session as AiSession & { title?: string | null }).title);
      status = { ok: true, text: t('newtab.prompt.started', { name, title }), session: session.id };
      text = '';
      requestAnimationFrame(autosize);
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
    <div class="bar">
      <!-- Bottom-left corner: kept free for the voice input (microphone) button. -->
      <div class="lead"></div>
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
        {#if offered}
          <label class="select model" title={modelHint}>
            <span class="sr-only">{t('newtab.prompt.model')}</span>
            <select value={model} onchange={(e) => chooseModel(e.currentTarget.value)}>
              <option value="">{defaultName ? t('newtab.prompt.modelDefault', { name: defaultName }) : t('newtab.prompt.modelDefaultPlain')}</option>
              {#each offered.models as m (m.id)}
                <option value={m.id} title={m.hint}>{m.name}</option>
              {/each}
            </select>
            <Icon name="chevron-down" size={12} stroke={2.25} />
          </label>
        {/if}
        <button class="btn primary" type="submit" disabled={!text.trim() || sending}>
          {sending ? t('newtab.prompt.sending') : t('newtab.prompt.start')}
          <kbd>Enter</kbd>
        </button>
      </div>
    </div>
  </form>
  {#if status}
    <div class="status" class:err={!status.ok} role="status">
      <Icon name={status.ok ? 'check' : 'x'} size={14} stroke={2.25} />
      <div class="status-text">
        <p class="msg">{status.text}</p>
        {#if status.ok}<p class="sub">{t('newtab.prompt.startedHint')}</p>{/if}
      </div>
      {#if status.ok}
        {@const session = status.session}
        <button class="btn" type="button" onclick={() => openAiPanel(session)}>{t('newtab.prompt.openPanel')}</button>
      {/if}
      <button class="icon-btn" type="button" aria-label={t('newtab.prompt.dismiss')} title={t('newtab.prompt.dismiss')} onclick={() => (status = null)}>
        <Icon name="x" size={14} />
      </button>
    </div>
  {:else}
    <div class="examples">
      {#each EXAMPLES as ex (ex)}
        <button
          class="ex"
          onclick={() => {
            text = ex;
            field.focus();
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
    flex-direction: column;
    gap: 6px;
    padding: 10px 10px 10px 18px;
    border: 1px solid var(--border-strong);
    border-radius: 16px;
    background: var(--surface);
    box-shadow: 0 6px 20px -10px rgb(14 17 22 / 0.3);
    transition: border-color 140ms, box-shadow 140ms;
  }
  .box:focus-within {
    border-color: var(--accent-line);
    box-shadow: 0 0 0 4px var(--accent-soft), 0 6px 20px -10px rgb(14 17 22 / 0.3);
  }
  textarea {
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
  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    min-height: 32px;
  }
  /* The microphone button goes here (left edge of the bottom row). */
  .lead {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 32px;
    margin-left: -8px;
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .select {
    position: relative;
    display: inline-flex;
    align-items: center;
    min-width: 0;
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
    max-width: 200px;
    padding: 0 26px 0 10px;
    border: 0;
    border-radius: var(--r);
    background: var(--sunken);
    color: var(--fg);
    font-size: var(--fs-sm);
    font-weight: 600;
    text-overflow: ellipsis;
  }
  .model select {
    font-weight: 500;
  }
  .btn.primary {
    height: 32px;
    padding: 0 8px 0 13px;
    flex: none;
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
    margin-top: 14px;
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
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 16px;
    padding: 10px 8px 10px 14px;
    border: 1px solid color-mix(in oklab, var(--ok) 30%, var(--border));
    border-radius: var(--r-lg);
    background: color-mix(in oklab, var(--ok) 6%, var(--surface));
    color: var(--ok);
    font-size: var(--fs-sm);
  }
  .status.err {
    border-color: color-mix(in oklab, var(--danger) 30%, var(--border));
    background: color-mix(in oklab, var(--danger) 6%, var(--surface));
    color: var(--danger);
  }
  .status > :global(svg) {
    flex: none;
  }
  .status-text {
    flex: 1;
    min-width: 0;
  }
  .status p {
    margin: 0;
    user-select: text;
  }
  .msg {
    color: var(--fg);
    font-weight: 600;
    overflow-wrap: anywhere;
  }
  .sub {
    margin-top: 2px;
    color: var(--fg-muted);
  }
  .status .btn {
    flex: none;
  }
  @media (max-width: 560px) {
    .bar {
      flex-wrap: wrap;
    }
    .controls {
      flex: 1;
      justify-content: flex-end;
      flex-wrap: wrap;
    }
    .status {
      flex-wrap: wrap;
    }
  }
</style>
