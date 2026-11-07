<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import Favicon from '../lib/components/Favicon.svelte';
  import { t, tp } from '../lib/i18n/index.svelte';
  import { MAX_PAGES, parsePages, startupErrors, startupForm, startupPatch, type StartupForm } from '../lib/settings/e1-startup';
  import type { ClosedTab, NewTabPosition, OnStartup, StartupConfig } from '../lib/settings/e1-types';
  import { isNewTab } from '../lib/urls';
  import Toggle from './Toggle.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  const STARTUP: OnStartup[] = ['restore', 'new_tab', 'pages'];
  const POSITIONS: NewTabPosition[] = ['end', 'after_active'];
  const NEW_TAB_MODES: StartupForm['new_tab_mode'][] = ['builtin', 'blank', 'url'];

  let saved = $state<StartupConfig | null>(null);
  let form = $state<StartupForm | null>(null);
  let loadError = $state<string | null>(null);
  let saving = $state(false);
  let closed = $state<ClosedTab[]>([]);

  async function load() {
    try {
      const r = await api.call('startup.config');
      saved = r.config;
      form = startupForm(r.config);
      loadError = null;
    } catch (err) {
      loadError = err instanceof Error ? err.message : String(err);
    }
  }

  async function loadClosed() {
    try {
      closed = (await api.call('tabs.closed')).tabs;
    } catch {
      closed = [];
    }
  }

  $effect(() => {
    void load();
    void loadClosed();
    // The list follows tabs being closed while the page is open.
    const off = api.on('tab.closed', () => setTimeout(() => void loadClosed(), 300));
    return off;
  });

  const errors = $derived(form ? startupErrors(form) : {});
  const patch = $derived(saved && form ? startupPatch(saved, form) : {});
  const dirty = $derived(Object.keys(patch).length > 0);
  const invalid = $derived(Object.keys(errors).length > 0);
  const pageCount = $derived(form ? parsePages(form.pagesText).pages.length : 0);

  async function save() {
    if (!dirty || invalid) return;
    saving = true;
    try {
      const r = await api.call('startup.config', patch);
      saved = r.config;
      form = startupForm(r.config);
      notify(t('settings.startup.saved'));
    } catch (err) {
      fail(err);
    } finally {
      saving = false;
    }
  }

  async function useOpenTabs() {
    try {
      const { tabs } = await api.call('tabs.list');
      const urls = tabs.map((x) => x.url).filter((u) => /^https?:/i.test(u) && !isNewTab(u));
      if (!urls.length) {
        notify(t('settings.startup.noOpenTabs'), true);
        return;
      }
      form!.pagesText = [...new Set(urls)].slice(0, MAX_PAGES).join('\n');
    } catch (err) {
      fail(err);
    }
  }

  async function reopen(tab: ClosedTab) {
    try {
      await api.call('tabs.closed_reopen', { index: tab.index });
      notify(t('settings.startup.reopened', { title: tab.title || tab.url }));
    } catch (err) {
      fail(err);
    }
    void loadClosed();
  }

  async function clearClosed() {
    try {
      const { cleared } = await api.call('tabs.closed_clear');
      notify(tp('settings.startup.closedCleared', cleared));
    } catch (err) {
      fail(err);
    }
    void loadClosed();
  }

  const errorText = (e: string | undefined) =>
    e === 'bad' ? t('settings.startup.err.bad') : e === 'empty' ? t('settings.startup.err.empty') : e === 'tooMany' ? t('settings.startup.err.tooMany', { n: MAX_PAGES }) : '';
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.startup.title')}</h2>
    <p>{t('settings.startup.intro')}</p>
  </header>

  {#if loadError && !form}
    <p class="lib-error" role="alert">{t('settings.startup.loadError', { error: loadError })}</p>
    <button class="btn" onclick={load}>{t('settings.startup.retry')}</button>
  {:else if !form}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="st-start">
      <h3 id="st-start">{t('settings.startup.onStartup')}</h3>
      <p class="lead">{t('settings.startup.onStartupLead')}</p>
      <div class="choices" role="radiogroup" aria-label={t('settings.startup.onStartup')}>
        {#each STARTUP as id (id)}
          <label class="choice">
            <input type="radio" name="on-startup" value={id} checked={form.on_startup === id} onchange={() => (form!.on_startup = id)} />
            <span class="text">
              <span class="label">{t(`settings.startup.mode.${id}`)}</span>
              <span class="help">{t(`settings.startup.mode.${id}.help`)}</span>
            </span>
          </label>
        {/each}
      </div>
      {#if form.on_startup === 'pages'}
        <div class="set-row stack">
          <div class="text">
            <span class="label">{t('settings.startup.pages')}</span>
            <span class="help">{t('settings.startup.pagesHelp', { n: MAX_PAGES })}</span>
          </div>
          <textarea
            class="lib-input pages"
            rows="4"
            spellcheck="false"
            bind:value={form.pagesText}
            placeholder="https://mail.example.com&#10;example.org"
            aria-label={t('settings.startup.pages')}
            aria-invalid={!!errors.pages}
          ></textarea>
          <div class="row-actions">
            <button class="btn" onclick={useOpenTabs}>{t('settings.startup.useOpenTabs')}</button>
            <span class="set-muted">{tp('settings.startup.pageCount', pageCount)}</span>
          </div>
          {#if errors.pages}<p class="set-err">{errorText(errors.pages)}</p>{/if}
        </div>
      {/if}
    </section>

    <section class="set-card" aria-labelledby="st-home">
      <h3 id="st-home">{t('settings.startup.homeAndNewTab')}</h3>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.startup.showHome')}</span><span class="help">{t('settings.startup.showHomeHelp')}</span></div>
        <div class="ctl"><Toggle label={t('settings.startup.showHome')} checked={form.show_home_button} onchange={(on) => (form!.show_home_button = on)} /></div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.startup.homePage')}</span><span class="help">{t('settings.startup.homePageHelp')}</span></div>
        <div class="ctl">
          <input
            class="lib-input wide"
            type="url"
            spellcheck="false"
            bind:value={form.home_page}
            placeholder={t('settings.startup.homePagePlaceholder')}
            aria-label={t('settings.startup.homePage')}
            aria-invalid={!!errors.home_page}
          />
        </div>
      </div>
      {#if errors.home_page}<p class="set-err pad">{errorText(errors.home_page)}</p>{/if}
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.startup.newTabPage')}</span><span class="help">{t(`settings.startup.newTab.${form.new_tab_mode}.help`)}</span></div>
        <div class="ctl">
          <div class="set-seg" role="radiogroup" aria-label={t('settings.startup.newTabPage')}>
            {#each NEW_TAB_MODES as m (m)}
              <button role="radio" aria-checked={form.new_tab_mode === m} onclick={() => (form!.new_tab_mode = m)}>{t(`settings.startup.newTab.${m}`)}</button>
            {/each}
          </div>
        </div>
      </div>
      {#if form.new_tab_mode === 'url'}
        <div class="set-row">
          <div class="text"><span class="label">{t('settings.startup.newTabUrl')}</span></div>
          <div class="ctl">
            <input
              class="lib-input wide"
              type="url"
              spellcheck="false"
              bind:value={form.new_tab_url}
              placeholder="https://example.com"
              aria-label={t('settings.startup.newTabUrl')}
              aria-invalid={!!errors.new_tab_url}
            />
          </div>
        </div>
        {#if errors.new_tab_url}<p class="set-err pad">{errorText(errors.new_tab_url)}</p>{/if}
      {/if}
    </section>

    <section class="set-card" aria-labelledby="st-tabs">
      <h3 id="st-tabs">{t('settings.startup.tabs')}</h3>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.startup.position')}</span><span class="help">{t('settings.startup.positionHelp')}</span></div>
        <div class="ctl">
          <div class="set-seg" role="radiogroup" aria-label={t('settings.startup.position')}>
            {#each POSITIONS as p (p)}
              <button role="radio" aria-checked={form.new_tab_position === p} onclick={() => (form!.new_tab_position = p)}>{t(`settings.startup.position.${p}`)}</button>
            {/each}
          </div>
        </div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.startup.keepWindow')}</span><span class="help">{t('settings.startup.keepWindowHelp')}</span></div>
        <div class="ctl"><Toggle label={t('settings.startup.keepWindow')} checked={form.keep_window_on_last_tab} onchange={(on) => (form!.keep_window_on_last_tab = on)} /></div>
      </div>
    </section>

    <section class="set-card" aria-labelledby="st-closed">
      <h3 id="st-closed"><span class="grow">{t('settings.startup.closed')}</span></h3>
      <p class="lead">{t('settings.startup.closedLead')}</p>
      {#if closed.length}
        <ul class="closed">
          {#each closed as c (c.index)}
            <li>
              <Favicon url={c.url} />
              <span class="t" title={c.url}>{c.title || c.url}</span>
              {#if c.profile}<span class="lib-chip">{c.profile}</span>{/if}
              <button class="btn small" onclick={() => reopen(c)}>{t('settings.startup.reopen')}</button>
            </li>
          {/each}
        </ul>
        <div class="set-actions">
          <button class="btn" onclick={clearClosed}>{t('settings.startup.clearClosed')}</button>
        </div>
      {:else}
        <p class="set-muted pad">{t('settings.startup.closedEmpty')}</p>
      {/if}
    </section>

    {#if dirty}
      <div class="set-savebar" role="region" aria-label={t('settings.startup.unsaved')}>
        <span>{invalid ? t('settings.startup.fixToSave') : t('settings.startup.unsaved')}</span>
        <button class="btn" disabled={saving} onclick={() => saved && (form = startupForm(saved))}>{t('settings.startup.discard')}</button>
        <button class="btn primary" disabled={saving || invalid} onclick={save}>{saving ? t('settings.startup.saving') : t('common.save')}</button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .pad {
    padding: 0 16px 12px;
  }
  .choices {
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--border);
  }
  .choice {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 16px;
    cursor: pointer;
  }
  .choice + .choice {
    border-top: 1px solid var(--border);
  }
  .choice input {
    margin-top: 3px;
    accent-color: var(--accent);
  }
  .choice .text {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .choice .label {
    font-weight: 550;
  }
  .choice .help {
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
    line-height: 1.45;
  }
  .pages {
    width: 100%;
    min-height: 84px;
    padding: 6px 8px;
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    resize: vertical;
  }
  .row-actions {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .closed {
    display: flex;
    flex-direction: column;
    margin: 0;
    padding: 0;
    border-top: 1px solid var(--border);
    list-style: none;
  }
  .closed li {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 16px;
  }
  .closed li + li {
    border-top: 1px solid var(--border);
  }
  .closed .t {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .btn.small {
    height: 26px;
    padding: 0 10px;
    font-size: var(--fs-sm);
  }
</style>
