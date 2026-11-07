<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import { locale, t, tp } from '../lib/i18n/index.svelte';
  import { areaLabel, bytesLabel, CLEAR_PARTS, share, sinceFor, TIME_RANGES, type ClearPart, type TimeRange } from '../lib/settings/e1-data';
  import { openLibraryFrom } from '../lib/settings/e1-open';
  import type { ClearResult, DataUsage, UsageArea } from '../lib/settings/e1-types';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  let range = $state<TimeRange>('hour');
  let parts = $state<Record<ClearPart, boolean>>({ history: true, downloads: false, cookies: true, cache: true, site_data: false });
  let profiles = $state<{ id: string; label: string; launch: boolean }[]>([]);
  let profile = $state<string>('');
  /** Second click needed (clearing cannot be undone). */
  let confirming = $state(false);
  let clearing = $state(false);
  let result = $state<ClearResult | null>(null);
  let usage = $state<DataUsage | null>(null);
  let usageError = $state<string | null>(null);
  let measuring = $state(false);

  async function loadProfiles() {
    try {
      const list = (await api.call('profile.list')).profiles.filter((p) => !p.incognito);
      profiles = list.map((p) => ({ id: p.id, label: p.label || p.id, launch: !!p.launch }));
      if (!profile) profile = list.find((p) => p.launch)?.id ?? list[0]?.id ?? '';
    } catch {
      profiles = [];
    }
  }

  async function measure() {
    measuring = true;
    try {
      usage = await api.call('browsing_data.usage');
      usageError = null;
    } catch (err) {
      usageError = err instanceof Error ? err.message : String(err);
    } finally {
      measuring = false;
    }
  }

  $effect(() => {
    void loadProfiles();
    void measure();
  });

  const chosen = $derived(CLEAR_PARTS.filter((p) => parts[p]));
  const perProfile = $derived(parts.cookies || parts.cache || parts.site_data);

  async function clear() {
    if (!chosen.length) return;
    if (!confirming) {
      confirming = true;
      return;
    }
    confirming = false;
    clearing = true;
    result = null;
    try {
      result = await api.call('browsing_data.clear', {
        since: sinceFor(range),
        profile: profile || undefined,
        history: parts.history,
        downloads: parts.downloads,
        cookies: parts.cookies,
        cache: parts.cache,
        site_data: parts.site_data,
      });
      if (result.errors.length) notify(t('settings.data.clearedWithErrors'), true);
      else notify(t('settings.data.cleared'));
      void measure();
    } catch (err) {
      fail(err);
    } finally {
      clearing = false;
    }
  }

  function areaName(a: UsageArea): string {
    const l = areaLabel(a);
    if (l.kind === 'profile') return t('settings.data.area.profile', { name: profiles.find((p) => p.id === l.profile)?.label ?? l.profile });
    if (l.kind === 'known') return t(`settings.data.area.${l.key}`);
    return l.name;
  }

  async function openFolder(path: string) {
    try {
      await api.call('system.open_path', { path });
    } catch (err) {
      fail(err);
    }
  }

  const size = (bytes: number) => bytesLabel(bytes, locale());
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.data.title')}</h2>
    <p>{t('settings.data.intro')}</p>
  </header>

  <section class="set-card" aria-labelledby="dt-clear">
    <h3 id="dt-clear">{t('settings.data.clearTitle')}</h3>
    <p class="lead">{t('settings.data.clearLead')}</p>
    <div class="set-row">
      <div class="text"><span class="label">{t('settings.data.range')}</span><span class="help">{t('settings.data.rangeHelp')}</span></div>
      <div class="ctl">
        <select class="lib-input" bind:value={range} aria-label={t('settings.data.range')} onchange={() => (confirming = false)}>
          {#each TIME_RANGES as r (r)}<option value={r}>{t(`settings.data.range.${r}`)}</option>{/each}
        </select>
      </div>
    </div>
    {#each CLEAR_PARTS as p (p)}
      <label class="set-row part">
        <input type="checkbox" bind:checked={parts[p]} onchange={() => (confirming = false)} />
        <span class="text"><span class="label">{t(`settings.data.part.${p}`)}</span><span class="help">{t(`settings.data.part.${p}.help`)}</span></span>
      </label>
    {/each}
    {#if perProfile && profiles.length > 1}
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.data.profile')}</span><span class="help">{t('settings.data.profileHelp')}</span></div>
        <div class="ctl">
          <select class="lib-input" bind:value={profile} aria-label={t('settings.data.profile')} onchange={() => (confirming = false)}>
            {#each profiles as p (p.id)}<option value={p.id}>{p.label}</option>{/each}
          </select>
        </div>
      </div>
    {/if}
    <div class="set-actions">
      <button class="btn" class:danger={confirming} disabled={clearing || !chosen.length} onclick={clear}>
        {clearing ? t('settings.data.clearing') : confirming ? t('settings.data.confirm') : t('settings.data.clearButton')}
      </button>
      {#if confirming}
        <button class="btn" onclick={() => (confirming = false)}>{t('common.cancel')}</button>
        <span class="set-muted">{t('settings.data.confirmHelp')}</span>
      {/if}
    </div>
    {#if result}
      <ul class="result" role="status">
        {#if result.history_visits != null}<li>{tp('settings.data.result.history', result.history_visits)}</li>{/if}
        {#if result.downloads != null}<li>{tp('settings.data.result.downloads', result.downloads)}</li>{/if}
        {#if result.cookies != null}<li>{tp('settings.data.result.cookies', result.cookies)}</li>{/if}
        {#if result.cache}<li>{t('settings.data.result.cache')}</li>{/if}
        {#if result.site_data_origins != null}<li>{tp('settings.data.result.sites', result.site_data_origins)}</li>{/if}
        {#each result.errors as e (e.part)}
          <li class="set-err">{t('settings.data.result.failed', { part: t(`settings.data.part.${e.part}`), error: e.error })}</li>
        {/each}
      </ul>
    {/if}
  </section>

  <section class="set-card" aria-labelledby="dt-usage">
    <h3 id="dt-usage"><span class="grow">{t('settings.data.storage')}</span></h3>
    <div class="set-row">
      <div class="text">
        <span class="label">{t('settings.data.folder')}</span>
        <span class="help">{t('settings.data.folderHelp')}</span>
        {#if usage}<span class="set-mono path">{usage.root}</span>{/if}
      </div>
      <div class="ctl">
        {#if usage}<button class="btn" onclick={() => openFolder(usage!.root)}>{t('settings.data.openFolder')}</button>{/if}
        <button class="btn" disabled={measuring} onclick={measure}>{measuring ? t('settings.data.measuring') : t('settings.data.measure')}</button>
      </div>
    </div>
    {#if usageError}
      <p class="set-err pad">{t('settings.data.usageError', { error: usageError })}</p>
    {:else if usage}
      <p class="set-muted pad">{t('settings.data.total', { size: size(usage.total_bytes) })}</p>
      <ul class="areas">
        {#each usage.areas.filter((a) => a.bytes > 0) as a (a.id)}
          <li>
            <span class="name" title={a.path}>{areaName(a)}</span>
            <span class="bar" aria-hidden="true"><span style:width="{share(a.bytes, usage.total_bytes)}%"></span></span>
            <span class="size">{size(a.bytes)}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  <section class="set-card" aria-labelledby="dt-move">
    <h3 id="dt-move">{t('settings.data.importTitle')}</h3>
    <div class="set-row">
      <div class="text"><span class="label">{t('settings.data.import')}</span><span class="help">{t('settings.data.importHelp')}</span></div>
      <div class="ctl"><button class="btn" onclick={() => openLibraryFrom(api, 'import').catch(fail)}>{t('settings.data.importButton')}</button></div>
    </div>
    <div class="set-row">
      <div class="text"><span class="label">{t('settings.data.backup')}</span><span class="help">{t('settings.data.backupHelp')}</span></div>
    </div>
  </section>
</div>

<style>
  .pad {
    padding: 0 16px 12px;
  }
  .part {
    cursor: pointer;
  }
  .part input {
    align-self: flex-start;
    margin-top: 3px;
    accent-color: var(--accent);
  }
  .btn.danger {
    border-color: var(--danger);
    background: var(--danger);
    color: #fff;
  }
  .result {
    margin: 0;
    padding: 0 16px 12px 34px;
    font-size: var(--fs-sm);
    line-height: 1.6;
  }
  .path {
    overflow-wrap: anywhere;
    color: var(--fg-muted);
  }
  .areas {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0;
    padding: 0 16px 14px;
    list-style: none;
  }
  .areas li {
    display: grid;
    grid-template-columns: minmax(120px, 220px) minmax(0, 1fr) 84px;
    align-items: center;
    gap: 12px;
    font-size: var(--fs-sm);
  }
  .areas .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .bar {
    height: 8px;
    overflow: hidden;
    border-radius: 4px;
    background: var(--sunken);
  }
  .bar span {
    display: block;
    height: 100%;
    border-radius: 4px;
    background: var(--accent);
  }
  .size {
    color: var(--fg-muted);
    font-variant-numeric: tabular-nums;
    text-align: right;
  }
</style>
