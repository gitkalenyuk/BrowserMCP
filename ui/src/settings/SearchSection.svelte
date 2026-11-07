<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import Icon from '../lib/components/Icon.svelte';
  import { t } from '../lib/i18n/index.svelte';
  import { engineErrors, intentWithEngines, omniboxSearch, resolveWithEngines, type EngineDraft } from '../lib/settings/e1-search';
  import type { SearchArgs, SearchConfig, SearchEngine } from '../lib/settings/e1-types';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  let config = $state<SearchConfig | null>(null);
  let loadError = $state<string | null>(null);
  let busy = $state(false);
  /** The engine being edited (null = the form adds a new one). */
  let editing = $state<string | null>(null);
  let draft = $state<EngineDraft>({ name: '', url: '', keyword: '' });
  let touched = $state(false);
  let tryText = $state('');

  async function load() {
    try {
      config = await api.call('search.config');
      loadError = null;
    } catch (err) {
      loadError = err instanceof Error ? err.message : String(err);
    }
  }

  $effect(() => {
    void load();
    return api.on('settings.changed', (e) => {
      if (e.area === 'search') void load();
    });
  });

  const errors = $derived(config ? engineErrors(draft, config.engines, editing) : {});
  const invalid = $derived(Object.keys(errors).length > 0);
  const search = $derived(omniboxSearch(config));
  const tryUrl = $derived(tryText.trim() ? resolveWithEngines(tryText, search) : null);
  const tryIntent = $derived(intentWithEngines(tryText, search));

  async function change(args: SearchArgs, done: string) {
    busy = true;
    try {
      config = await api.call('search.config', args);
      notify(done);
      return true;
    } catch (err) {
      fail(err);
      return false;
    } finally {
      busy = false;
    }
  }

  function edit(e: SearchEngine) {
    editing = e.id;
    draft = { name: e.name, url: e.url, keyword: e.keyword };
    touched = false;
  }

  function resetForm() {
    editing = null;
    draft = { name: '', url: '', keyword: '' };
    touched = false;
  }

  async function submit() {
    touched = true;
    if (invalid) return;
    const input = { name: draft.name.trim(), url: draft.url.trim(), keyword: draft.keyword.trim() || null };
    const ok = editing
      ? await change({ update: { id: editing, ...input } }, t('settings.search.updated', { name: input.name }))
      : await change({ add: input }, t('settings.search.added', { name: input.name }));
    if (ok) resetForm();
  }

  const errorText = (field: keyof EngineDraft): string => {
    const e = errors[field];
    return e ? t(`settings.search.err.${field}.${e}`) : '';
  };
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.search.title')}</h2>
    <p>{t('settings.search.intro')}</p>
  </header>

  {#if loadError && !config}
    <p class="lib-error" role="alert">{t('settings.search.loadError', { error: loadError })}</p>
    <button class="btn" onclick={load}>{t('settings.search.retry')}</button>
  {:else if !config}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="se-engines">
      <h3 id="se-engines">{t('settings.search.engines')}</h3>
      <p class="lead">{t('settings.search.enginesLead')}</p>
      <div class="set-table-wrap">
        <table class="set-table">
          <thead>
            <tr>
              <th class="c">{t('settings.search.col.default')}</th>
              <th>{t('settings.search.col.name')}</th>
              <th>{t('settings.search.col.keyword')}</th>
              <th>{t('settings.search.col.url')}</th>
              <th class="r"><span class="sr">{t('settings.search.col.actions')}</span></th>
            </tr>
          </thead>
          <tbody>
            {#each config.engines as e (e.id)}
              <tr>
                <td class="c">
                  <input
                    type="radio"
                    name="default-engine"
                    checked={config.default === e.id}
                    disabled={busy}
                    aria-label={t('settings.search.makeDefault', { name: e.name })}
                    onchange={() => change({ default: e.id }, t('settings.search.defaultSet', { name: e.name }))}
                  />
                </td>
                <td class="name">
                  {e.name}
                  {#if config.default === e.id}<span class="lib-chip">{t('settings.search.defaultChip')}</span>{/if}
                  {#if !e.builtin}<span class="lib-chip">{t('settings.search.customChip')}</span>{/if}
                </td>
                <td><kbd>{e.keyword}</kbd></td>
                <td class="set-mono url" title={e.url}>{e.url}</td>
                <td class="r">
                  {#if !e.builtin}
                    <button class="icon-btn" aria-label={t('settings.search.edit', { name: e.name })} title={t('common.edit')} onclick={() => edit(e)}><Icon name="pencil" size={14} /></button>
                    <button
                      class="icon-btn"
                      aria-label={t('settings.search.remove', { name: e.name })}
                      title={t('common.delete')}
                      disabled={busy}
                      onclick={() => change({ remove: e.id }, t('settings.search.removed', { name: e.name }))}
                    ><Icon name="trash" size={14} /></button>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </section>

    <section class="set-card" aria-labelledby="se-add">
      <h3 id="se-add">{editing ? t('settings.search.editTitle') : t('settings.search.addTitle')}</h3>
      <p class="lead">{t('settings.search.addLead')}</p>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.search.field.name')}</span></div>
        <div class="ctl">
          <input class="lib-input wide" bind:value={draft.name} placeholder={t('settings.search.field.namePlaceholder')} aria-label={t('settings.search.field.name')} aria-invalid={touched && !!errors.name} />
        </div>
      </div>
      {#if touched && errors.name}<p class="set-err pad">{errorText('name')}</p>{/if}
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.search.field.url')}</span><span class="help">{t('settings.search.field.urlHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input wide set-mono" type="url" spellcheck="false" bind:value={draft.url} placeholder="https://example.com/search?q=%s" aria-label={t('settings.search.field.url')} aria-invalid={touched && !!errors.url} />
        </div>
      </div>
      {#if touched && errors.url}<p class="set-err pad">{errorText('url')}</p>{/if}
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.search.field.keyword')}</span><span class="help">{t('settings.search.field.keywordHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input narrow" spellcheck="false" bind:value={draft.keyword} placeholder="w" aria-label={t('settings.search.field.keyword')} aria-invalid={touched && !!errors.keyword} />
        </div>
      </div>
      {#if touched && errors.keyword}<p class="set-err pad">{errorText('keyword')}</p>{/if}
      <div class="set-actions">
        <button class="btn primary" disabled={busy || (touched && invalid)} onclick={submit}>{editing ? t('common.save') : t('settings.search.addButton')}</button>
        {#if editing || draft.name || draft.url || draft.keyword}
          <button class="btn" disabled={busy} onclick={resetForm}>{t('common.cancel')}</button>
        {/if}
      </div>
    </section>

    <section class="set-card" aria-labelledby="se-try">
      <h3 id="se-try">{t('settings.search.tryTitle')}</h3>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.search.tryLabel')}</span><span class="help">{t('settings.search.tryHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input wide" bind:value={tryText} placeholder={t('settings.search.tryPlaceholder')} aria-label={t('settings.search.tryLabel')} />
        </div>
      </div>
      {#if tryUrl}
        <p class="set-muted pad" role="status">
          {tryIntent?.kind === 'search' ? t('settings.search.trySearch', { engine: tryIntent.engine }) : t('settings.search.tryOpen')}
          <span class="set-mono">{tryUrl}</span>
        </p>
      {/if}
    </section>
  {/if}
</div>

<style>
  .pad {
    padding: 0 16px 12px;
    overflow-wrap: anywhere;
  }
  .url {
    max-width: 320px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  td .lib-chip {
    margin-left: 6px;
  }
  .name {
    white-space: nowrap;
  }
  td input[type='radio'] {
    accent-color: var(--accent);
  }
  kbd {
    padding: 1px 6px;
    border: 1px solid var(--border-strong);
    border-radius: 4px;
    background: var(--sunken);
    font: 12px var(--font-mono);
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
</style>
