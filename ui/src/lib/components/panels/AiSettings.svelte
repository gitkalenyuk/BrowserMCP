<script lang="ts">
  import type { AiProvider } from '../../api';
  import type { AiStore } from '../../ai/store.svelte';
  import { isUser } from '../../ai/provider-form';
  import { t } from '../../i18n/index.svelte';
  import { BLANK_TEMPLATE, providerHint } from '../../providers';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';
  import ProviderForm from './ProviderForm.svelte';

  /**
   * Provider settings: CLI agents, built-in API providers and the user's own ("My providers"),
   * one expandable row each. `store`: the settings page has no chrome controller and passes its
   * own AiStore; the side panel uses the chrome's. Without `onback` there is no back bar.
   */
  let { onback, store }: { onback?: () => void; store?: AiStore } = $props();

  // svelte-ignore state_referenced_locally
  const ai = store ?? useChrome().ai;

  /** `openId` of the "add provider" form. */
  const NEW = '\u0000new';
  // svelte-ignore state_referenced_locally
  let openId = $state<string | null>(ai.provider);
  /** The open form holds edits that are not saved. */
  let dirty = $state(false);
  /** What the user wanted to do while the open form was dirty; runs on "Discard changes". */
  let pending = $state<(() => void) | null>(null);
  /** Template of the add form: a built-in API provider id or BLANK_TEMPLATE. */
  let templateId = $state('openai');
  /** First message of the next opened form (e.g. "Added" after creating a provider). */
  let notice = $state<string | null>(null);

  const cli = $derived(ai.providers.filter((p) => p.kind === 'cli'));
  const builtinApi = $derived(ai.providers.filter((p) => p.kind === 'api' && !isUser(p)));
  const mine = $derived(ai.providers.filter(isUser));
  const template = $derived(templateId === BLANK_TEMPLATE ? null : (builtinApi.find((p) => p.id === templateId) ?? null));
  const openName = $derived(ai.providers.find((p) => p.id === openId)?.name ?? '');

  /** Run `action` now, or after the user agrees to drop the open form's edits. */
  function guard(action: () => void) {
    if (dirty) pending = action;
    else action();
  }

  function discard() {
    const action = pending;
    pending = null;
    dirty = false;
    action?.();
  }

  function toggle(id: string) {
    guard(() => {
      openId = openId === id ? null : id;
      notice = null;
    });
  }

  function chooseTemplate(select: HTMLSelectElement) {
    const next = select.value;
    // Keep showing the current template until the user decides about the typed draft.
    select.value = templateId;
    guard(() => (templateId = next));
  }

  function created(p: AiProvider) {
    dirty = false;
    openId = p.id;
    notice = t('aiProviders.new.done');
  }

  function closed() {
    dirty = false;
    openId = null;
  }

  // The question shows on the open form, which may be far from what was clicked: bring it in view.
  let unsavedBar = $state<HTMLElement>();
  $effect(() => {
    unsavedBar?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  });

  // Leaving the page with unsaved edits asks first.
  $effect(() => {
    if (!dirty) return;
    const onUnload = (e: BeforeUnloadEvent) => e.preventDefault();
    window.addEventListener('beforeunload', onUnload);
    return () => window.removeEventListener('beforeunload', onUnload);
  });

  function badge(p: AiProvider): { text: string; ready: boolean } {
    if (p.configured) return { text: t('aiProviders.badge.ready'), ready: true };
    if (p.kind === 'cli') return { text: t('aiProviders.badge.notFound'), ready: false };
    if (!p.base_url) return { text: t('aiProviders.badge.noUrl'), ready: false };
    return { text: t('aiProviders.badge.noKey'), ready: false };
  }
</script>

{#snippet unsaved()}
  {#if pending}
    <div class="unsaved" role="alert" bind:this={unsavedBar}>
      <span>{openId === NEW ? t('aiProviders.unsavedNew') : t('aiProviders.unsaved', { name: openName })}</span>
      <button class="btn" onclick={() => (pending = null)}>{t('aiProviders.unsavedKeep')}</button>
      <button class="btn danger" onclick={discard}>{t('aiProviders.unsavedDiscard')}</button>
    </div>
  {/if}
{/snippet}

{#snippet row(p: AiProvider)}
  {@const b = badge(p)}
  <section class="prov" class:open={openId === p.id}>
    <button class="row" aria-expanded={openId === p.id} onclick={() => toggle(p.id)}>
      <span class="name">{p.name}</span>
      <span class="hint">{p.kind === 'cli' ? t('aiProviders.hint.cli') : providerHint(p.id)}</span>
      {#if p.is_default}<span class="badge def" title={t('aiProviders.badge.defaultTitle')}>{t('aiProviders.badge.default')}</span>{/if}
      <span class="badge" class:ready={b.ready}>{b.text}</span>
      {#if ai.provider === p.id}<span class="sel" title={t('aiProviders.inUseTitle')}><Icon name="check" size={13} stroke={2.25} /></span>{/if}
    </button>
    {#if openId === p.id}
      {@render unsaved()}
      <ProviderForm {ai} provider={p} notice={openId === p.id ? notice : null} bind:dirty onremoved={closed} />
      {#if p.kind === 'cli'}
        <!-- CLI agents' own details (e.g. Claude Code / Codex permissions: AgentAccess.svelte)
             render here, under the provider form. -->
        <div class="cli-details" data-provider={p.id}></div>
      {/if}
    {/if}
  </section>
{/snippet}

<div class="settings" class:page={!onback}>
  {#if onback}
    <div class="bar">
      <button class="icon-btn" aria-label={t('aiProviders.back')} title={t('common.back')} onclick={() => guard(() => onback?.())}>
        <Icon name="arrow-left" size={15} />
      </button>
      <h3>{t('aiProviders.title')}</h3>
    </div>
  {/if}
  <div class="list">
    {#if ai.providersError}
      <p class="err">{t('aiProviders.loadError', { error: ai.providersError })}</p>
    {/if}

    {#if ai.providers.length}
      {#if cli.length}
        <div class="group">
          <h4>{t('aiProviders.group.agents')}</h4>
          <p>{t('aiProviders.group.agentsNote')}</p>
        </div>
        {#each cli as p (p.id)}{@render row(p)}{/each}
      {/if}

      <div class="group">
        <h4>{t('aiProviders.group.builtin')}</h4>
        <p>{t('aiProviders.group.builtinNote')}</p>
      </div>
      {#each builtinApi as p (p.id)}{@render row(p)}{/each}

      <div class="group">
        <div class="group-head">
          <h4>{t('aiProviders.group.mine')}</h4>
          <button class="btn" disabled={openId === NEW} onclick={() => guard(() => ((openId = NEW), (notice = null)))}>
            <Icon name="plus" size={13} />
            {t('aiProviders.new.open')}
          </button>
        </div>
        <p>{t('aiProviders.group.mineNote')}</p>
      </div>
      {#if openId === NEW}
        <section class="prov open new" aria-label={t('aiProviders.new.title')}>
          {@render unsaved()}
          <label class="template">
            <span>{t('aiProviders.new.template')}</span>
            <select value={templateId} onchange={(e) => chooseTemplate(e.currentTarget)}>
              {#each builtinApi as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
              <option value={BLANK_TEMPLATE}>{t('aiProviders.new.blank')}</option>
            </select>
          </label>
          {#key templateId}
            <ProviderForm {ai} {template} bind:dirty oncreated={created} oncancel={() => guard(closed)} />
          {/key}
        </section>
      {/if}
      {#each mine as p (p.id)}{@render row(p)}{/each}
      {#if !mine.length && openId !== NEW}
        <p class="muted">{t('aiProviders.mine.empty')}</p>
      {/if}
    {:else if !ai.providersError}
      <p class="muted">{t('aiProviders.loading')}</p>
    {/if}
  </div>
</div>

<style>
  .settings {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 44px;
    padding: 6px 10px 6px 6px;
    border-bottom: 1px solid var(--border);
  }
  h3 {
    margin: 0;
    font-size: var(--fs);
    font-weight: 650;
  }
  .list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 6px 0;
  }
  .settings.page .list {
    overflow: visible;
    padding: 0;
  }
  .group {
    padding: 14px 12px 4px;
  }
  .group h4 {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 650;
    letter-spacing: 0.02em;
    text-transform: uppercase;
  }
  .group p {
    margin: 2px 0 0;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    line-height: 1.45;
  }
  .group-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }
  .group-head .btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .prov.open {
    background: var(--surface);
    border-block: 1px solid var(--border);
  }
  .prov.new {
    margin-top: 6px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 34px;
    padding: 0 12px;
    border: 0;
    background: transparent;
    text-align: left;
  }
  .row:hover {
    background: var(--hover);
  }
  .name {
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 50%;
  }
  .hint {
    flex: 1;
    min-width: 0;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .badge {
    padding: 1px 7px;
    border-radius: 9px;
    background: var(--sunken);
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    font-weight: 600;
    white-space: nowrap;
  }
  .badge.def {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .badge.ready {
    background: color-mix(in oklab, var(--ok) 14%, transparent);
    color: var(--ok);
  }
  .sel {
    display: grid;
    color: var(--accent);
  }
  .template {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 10px 12px 4px;
    font-size: var(--fs-xs);
    font-weight: 600;
    color: var(--fg-muted);
  }
  .template select {
    height: 28px;
    padding: 0 4px 0 9px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--field-focus);
    color: var(--fg);
    font-size: var(--fs-sm);
    font-weight: 400;
  }
  .cli-details:empty {
    display: none;
  }
  .unsaved {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin: 8px 12px 2px;
    padding: 8px 10px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--surface);
    box-shadow: 0 2px 8px rgb(0 0 0 / 0.12);
    font-size: var(--fs-sm);
  }
  .unsaved span {
    flex: 1;
    min-width: 12ch;
  }
  .btn.danger {
    color: var(--danger);
  }
  .muted {
    margin: 0;
    padding: 8px 12px 12px;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
  .err {
    margin: 8px 12px;
    color: var(--danger);
    font-size: var(--fs-xs);
  }
</style>
