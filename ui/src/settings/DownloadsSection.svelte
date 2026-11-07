<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import { t, tp } from '../lib/i18n/index.svelte';
  import { openLibraryFrom } from '../lib/settings/e1-open';
  import type { DownloadsConfig } from '../lib/settings/e1-types';
  import Toggle from './Toggle.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  let config = $state<DownloadsConfig | null>(null);
  let loadError = $state<string | null>(null);
  let busy = $state(false);
  /** The folder being typed (null = not editing). */
  let editingDir = $state<string | null>(null);
  let count = $state<number | null>(null);

  async function load() {
    try {
      config = await api.call('downloads.config');
      loadError = null;
    } catch (err) {
      loadError = err instanceof Error ? err.message : String(err);
    }
    try {
      count = (await api.call('downloads.list', { limit: 5000 })).downloads.length;
    } catch {
      count = null;
    }
  }

  $effect(() => {
    void load();
  });

  async function set(args: { dir?: string; ask?: boolean }, done: string) {
    busy = true;
    try {
      config = await api.call('downloads.config', args);
      notify(done);
      return true;
    } catch (err) {
      fail(err);
      return false;
    } finally {
      busy = false;
    }
  }

  async function saveDir() {
    const dir = editingDir?.trim() ?? '';
    if (!dir) return;
    if (await set({ dir }, t('settings.downloads.dirSaved'))) editingDir = null;
  }

  async function openFolder() {
    if (!config) return;
    try {
      await api.call('system.open_path', { path: config.dir });
    } catch (err) {
      fail(err);
    }
  }

  async function clearList() {
    busy = true;
    try {
      const { removed } = await api.call('downloads.clear');
      notify(tp('settings.downloads.cleared', removed));
      count = (await api.call('downloads.list', { limit: 5000 })).downloads.length;
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.downloads.title')}</h2>
    <p>{t('settings.downloads.intro')}</p>
  </header>

  {#if loadError && !config}
    <p class="lib-error" role="alert">{t('settings.downloads.loadError', { error: loadError })}</p>
    <button class="btn" onclick={load}>{t('settings.downloads.retry')}</button>
  {:else if !config}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="dl-where">
      <h3 id="dl-where">{t('settings.downloads.location')}</h3>
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.downloads.folder')}</span>
          <span class="help">{config.custom ? t('settings.downloads.folderCustomHelp') : t('settings.downloads.folderDefaultHelp')}</span>
          {#if editingDir === null}<span class="set-mono path" title={config.dir}>{config.dir}</span>{/if}
        </div>
        <div class="ctl">
          {#if editingDir === null}
            <button class="btn" onclick={openFolder}>{t('settings.downloads.openFolder')}</button>
            <button class="btn" onclick={() => (editingDir = config!.dir)}>{t('settings.downloads.change')}</button>
            {#if config.custom}
              <button class="btn" disabled={busy} onclick={() => set({ dir: '' }, t('settings.downloads.dirReset'))}>{t('settings.downloads.useDefault')}</button>
            {/if}
          {/if}
        </div>
      </div>
      {#if editingDir !== null}
        <div class="set-row stack">
          <input
            class="lib-input set-mono"
            spellcheck="false"
            bind:value={editingDir}
            placeholder={t('settings.downloads.folderPlaceholder')}
            aria-label={t('settings.downloads.folder')}
            onkeydown={(e) => {
              if (e.key === 'Enter') void saveDir();
              if (e.key === 'Escape') editingDir = null;
            }}
          />
          <p class="set-muted">{t('settings.downloads.folderTypeHelp', { example: config.default_dir })}</p>
          <div class="row-actions">
            <button class="btn primary" disabled={busy || !editingDir.trim()} onclick={saveDir}>{t('common.save')}</button>
            <button class="btn" disabled={busy} onclick={() => (editingDir = null)}>{t('common.cancel')}</button>
          </div>
        </div>
      {/if}
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.downloads.ask')}</span><span class="help">{t('settings.downloads.askHelp')}</span></div>
        <div class="ctl">
          <Toggle
            label={t('settings.downloads.ask')}
            checked={config.ask}
            disabled={busy}
            onchange={(on) => set({ ask: on }, on ? t('settings.downloads.askOn') : t('settings.downloads.askOff'))}
          />
        </div>
      </div>
    </section>

    <section class="set-card" aria-labelledby="dl-list">
      <h3 id="dl-list">{t('settings.downloads.list')}</h3>
      <div class="set-row">
        <div class="text">
          <span class="label">{count === null ? t('settings.downloads.listUnknown') : tp('settings.downloads.count', count)}</span>
          <span class="help">{t('settings.downloads.listHelp')}</span>
        </div>
        <div class="ctl">
          <button class="btn" onclick={() => openLibraryFrom(api, 'downloads').catch(fail)}>{t('settings.downloads.showAll')}</button>
          <button class="btn" disabled={busy || count === 0} onclick={clearList}>{t('settings.downloads.clear')}</button>
        </div>
      </div>
      <p class="set-muted pad">{t('settings.downloads.runsNote')}</p>
    </section>
  {/if}
</div>

<style>
  .pad {
    padding: 0 16px 12px;
  }
  .path {
    overflow-wrap: anywhere;
    color: var(--fg-muted);
  }
  .row-actions {
    display: flex;
    gap: 8px;
  }
</style>
