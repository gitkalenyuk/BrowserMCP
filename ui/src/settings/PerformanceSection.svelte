<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { GovernorLimits, GovernorProfile, GovernorStatus } from '../lib/api/types-settings';
  import Favicon from '../lib/components/Favicon.svelte';
  import { locale, t } from '../lib/i18n/index.svelte';
  import { governorErrors, governorForm, governorPatch, idleLabel, mbLabel, withProfile, type GovernorForm } from '../lib/settings/forms';
  import { isEmptyPatch } from '../lib/settings/notify';
  import Toggle from './Toggle.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  const PROFILES: GovernorProfile[] = ['low', 'normal', 'high'];

  let status = $state<GovernorStatus | null>(null);
  let limits = $state<GovernorLimits | null>(null);
  let form = $state<GovernorForm | null>(null);
  let loadError = $state<string | null>(null);
  let saving = $state(false);

  async function refresh(resetForm = false) {
    try {
      const s = await api.call('governor.status');
      status = s;
      loadError = null;
      // Keep what the user is typing; take the limits in force when nothing was changed.
      if (resetForm || !form || !limits || isEmptyPatch(governorPatch(limits, form))) {
        limits = s.limits;
        form = governorForm(s.limits);
      }
    } catch (err) {
      loadError = (err as Error).message;
    }
  }

  $effect(() => {
    void refresh(true);
    const timer = setInterval(() => void refresh(), 3000);
    return () => clearInterval(timer);
  });

  const errors = $derived(form ? governorErrors(form) : {});
  const patch = $derived(limits && form ? governorPatch(limits, form) : {});
  const dirty = $derived(!isEmptyPatch(patch));
  const invalid = $derived(Object.keys(errors).length > 0);

  async function save() {
    if (!dirty || invalid) return;
    saving = true;
    try {
      const r = await api.call('governor.config', patch);
      limits = r.limits;
      form = governorForm(r.limits);
      notify(t('settings.perf.saved'));
      void refresh();
    } catch (err) {
      fail(err);
    } finally {
      saving = false;
    }
  }

  async function resetAll() {
    saving = true;
    try {
      const r = await api.call('governor.config', { reset: true });
      limits = r.limits;
      form = governorForm(r.limits);
      notify(t('settings.perf.resetDone', { profile: t(`settings.perf.profile.${r.limits.profile}`) }));
    } catch (err) {
      fail(err);
    } finally {
      saving = false;
    }
  }

  const hasOverrides = $derived(!!status && Object.values(status.overrides).some((v) => v !== null && v !== undefined));
  const counts = $derived.by(() => {
    const c = { live: 0, frozen: 0, discarded: 0 };
    for (const t of status?.tabs ?? []) c[t.state]++;
    return c;
  });
  const timeFmt = $derived(new Intl.DateTimeFormat(locale(), { hour: '2-digit', minute: '2-digit' }));
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.perf.title')}</h2>
    <p>{t('settings.perf.intro')}</p>
  </header>

  {#if loadError && !status}
    <p class="lib-error" role="alert">{t('settings.perf.loadError', { error: loadError })}</p>
    <button class="btn" onclick={() => refresh(true)}>{t('settings.hints.retry')}</button>
  {:else if !form || !status}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="p-pol">
      <h3 id="p-pol">{t('settings.perf.policy')}</h3>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.perf.enabled')}</span><span class="help">{t('settings.perf.enabledHelp')}</span></div>
        <div class="ctl"><Toggle label={t('settings.perf.enabled')} checked={form.enabled} onchange={(on) => (form!.enabled = on)} /></div>
      </div>
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.perf.profile')}</span>
          <span class="help">{t(`settings.perf.profileHelp.${form.profile}`)} {t('settings.perf.profileHint')}</span>
        </div>
        <div class="ctl">
          <div class="set-seg" role="radiogroup" aria-label={t('settings.perf.profile')}>
            {#each PROFILES as p (p)}
              <button role="radio" aria-checked={form.profile === p} onclick={() => (form = withProfile(form!, p))}>{t(`settings.perf.profile.${p}`)}</button>
            {/each}
          </div>
        </div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.perf.freeze')}</span><span class="help">{t('settings.perf.freezeHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input narrow" type="number" min="0" max="1440" step="0.5" bind:value={form.freezeMin} disabled={!form.enabled} aria-label={t('settings.perf.freezeLabel')} aria-invalid={!!errors.freezeMin} /> {t('settings.perf.minShort')}
        </div>
      </div>
      {#if errors.freezeMin}<p class="set-err pad">{errors.freezeMin}</p>{/if}
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.perf.discard')}</span><span class="help">{t('settings.perf.discardHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input narrow" type="number" min="0" max="1440" step="1" bind:value={form.discardMin} disabled={!form.enabled} aria-label={t('settings.perf.discardLabel')} aria-invalid={!!errors.discardMin} /> {t('settings.perf.minShort')}
        </div>
      </div>
      {#if errors.discardMin}<p class="set-err pad">{errors.discardMin}</p>{/if}
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.perf.maxTabs')}</span><span class="help">{t('settings.perf.maxTabsHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input narrow" type="number" min="0" max="500" step="1" bind:value={form.maxLiveTabs} disabled={!form.enabled} aria-label={t('settings.perf.maxTabs')} aria-invalid={!!errors.maxLiveTabs} />
        </div>
      </div>
      {#if errors.maxLiveTabs}<p class="set-err pad">{errors.maxLiveTabs}</p>{/if}
      {#if hasOverrides}
        <div class="set-actions">
          <button class="btn" disabled={saving} onclick={resetAll}>{t('settings.perf.reset')}</button>
          <span class="set-muted">{t('settings.perf.changed')} {Object.entries(status.overrides).filter(([, v]) => v !== null && v !== undefined).map(([k]) => k.replace(/_/g, ' ')).join(', ')}</span>
        </div>
      {/if}
    </section>

    <section class="set-card" aria-labelledby="p-mem">
      <h3 id="p-mem"><span class="grow">{t('settings.perf.now')}</span><span class="set-muted live" aria-hidden="true">{t('settings.perf.updates')}</span></h3>
      <div class="stats">
        <div><b>{mbLabel(status.memory.browser_mb)}</b><span>{t('settings.perf.usedByBrowser')}</span></div>
        <div class:warn={status.memory.pressure}>
          <b>{mbLabel(status.memory.available_mb)}</b><span>{t('settings.perf.freeOf', { total: mbLabel(status.memory.total_mb), pct: status.memory.free_pct })}{status.memory.pressure ? ` · ${t('settings.perf.lowMemory')}` : ''}</span>
        </div>
        <div><b>{counts.live}</b><span>{t('settings.perf.liveTabs')}{status.limits.max_live_tabs ? ` ${t('settings.perf.limit', { n: status.limits.max_live_tabs })}` : ''}</span></div>
        <div><b>{counts.frozen} / {counts.discarded}</b><span>{t('settings.perf.frozenDiscarded')}</span></div>
      </div>
      <div class="set-table-wrap">
        <table class="set-table">
          <thead><tr><th>{t('settings.perf.colTab')}</th><th>{t('settings.perf.colState')}</th><th class="r">{t('settings.perf.colIdle')}</th><th class="r">{t('settings.perf.colHeap')}</th><th>{t('settings.perf.colProtected')}</th></tr></thead>
          <tbody>
            {#each status.tabs as tab (tab.id)}
              <tr>
                <td class="tab">
                  <Favicon url={tab.url} />
                  <span class="t" title={tab.url}>{tab.title || tab.url}</span>
                </td>
                <td><span class="lib-chip state-{tab.state}">{t(`settings.perf.state.${tab.state}`)}</span></td>
                <td class="r">{tab.idle_secs ? idleLabel(tab.idle_secs) : t('settings.perf.active')}</td>
                <td class="r">{mbLabel(tab.js_heap_mb)}</td>
                <td class="set-muted">{tab.protected ?? '—'}</td>
              </tr>
            {:else}
              <tr><td colspan="5" class="set-muted">{t('settings.perf.noTabs')}</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if status.decisions.length}
        <div class="dec">
          <h4>{t('settings.perf.decisions')}</h4>
          <ul>
            {#each status.decisions.slice(-6).reverse() as d, i (i)}
              <li>
                <span class="set-mono">{timeFmt.format(d.ts_ms)}</span>
                <span class="lib-chip">{d.action}</span>
                {t('settings.perf.decisionTab', { tab: d.tab })} {d.reason}{d.error ? ` — ${d.error}` : ''}
              </li>
            {/each}
          </ul>
        </div>
      {/if}
    </section>

    {#if dirty}
      <div class="set-savebar" role="region" aria-label={t('settings.general.unsaved')}>
        <span>{invalid ? t('settings.general.fixToSave') : t('settings.general.unsaved')}</span>
        <button class="btn" disabled={saving} onclick={() => limits && (form = governorForm(limits))}>{t('settings.general.discard')}</button>
        <button class="btn primary" disabled={saving || invalid} onclick={save}>{saving ? t('settings.general.saving') : t('common.save')}</button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .pad {
    padding: 0 16px 10px;
  }
  .live {
    font-size: var(--fs-xs);
    font-weight: 400;
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 1px;
    margin: 6px 16px 12px;
    overflow: hidden;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--border);
  }
  .stats > div {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px 12px;
    background: var(--surface);
  }
  .stats b {
    font-size: 17px;
    font-weight: 650;
    font-variant-numeric: tabular-nums;
  }
  .stats span {
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
  .stats .warn b {
    color: var(--warn);
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: 360px;
  }
  .tab .t {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .state-live {
    color: var(--ok);
  }
  .state-frozen {
    color: var(--accent);
  }
  .state-discarded {
    color: var(--fg-subtle);
  }
  .dec {
    padding: 4px 16px 12px;
    border-top: 1px solid var(--border);
  }
  .dec h4 {
    margin: 8px 0 6px;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 650;
    letter-spacing: 0.03em;
    text-transform: uppercase;
  }
  .dec ul {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: var(--fs-sm);
  }
  .dec li {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  @media (max-width: 720px) {
    .stats {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
</style>
