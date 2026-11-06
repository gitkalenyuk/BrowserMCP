<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { GovernorLimits, GovernorProfile, GovernorStatus } from '../lib/api/types-settings';
  import Favicon from '../lib/components/Favicon.svelte';
  import { governorErrors, governorForm, governorPatch, idleLabel, mbLabel, withProfile, type GovernorForm } from '../lib/settings/forms';
  import { isEmptyPatch } from '../lib/settings/notify';
  import Toggle from './Toggle.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  const PROFILES: { id: GovernorProfile; label: string; help: string }[] = [
    { id: 'low', label: 'Low', help: '≤ 8 GB RAM: freeze after 2 min, discard after 10 min, 8 live tabs' },
    { id: 'normal', label: 'Normal', help: 'About 16 GB: freeze after 5 min, discard after 30 min, 20 live tabs' },
    { id: 'high', label: 'High', help: '≥ 32 GB: freeze after 10 min, discard after 60 min, 40 live tabs' },
  ];

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
      notify('Performance settings saved');
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
      notify(`Back to the ${r.limits.profile} profile defaults`);
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
  const timeFmt = new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' });
</script>

<div class="set-page">
  <header>
    <h2>Performance</h2>
    <p>
      Background tabs are frozen (paused, memory kept) and later discarded (unloaded; the address, title and scroll position come
      back when you open them). The active tab, tabs an agent works in and pages playing audio are never touched.
    </p>
  </header>

  {#if loadError && !status}
    <p class="lib-error" role="alert">Could not read the tab governor: {loadError}</p>
    <button class="btn" onclick={() => refresh(true)}>Retry</button>
  {:else if !form || !status}
    <p class="set-muted">Loading…</p>
  {:else}
    <section class="set-card" aria-labelledby="p-pol">
      <h3 id="p-pol">Policy</h3>
      <div class="set-row">
        <div class="text"><span class="label">Freeze and discard idle tabs</span><span class="help">Off: tabs stay loaded until you close them (agents can still freeze tabs on purpose).</span></div>
        <div class="ctl"><Toggle label="Freeze and discard idle tabs" checked={form.enabled} onchange={(on) => (form!.enabled = on)} /></div>
      </div>
      <div class="set-row">
        <div class="text">
          <span class="label">Profile</span>
          <span class="help">{PROFILES.find((p) => p.id === form!.profile)?.help} (picked from your RAM; choosing one fills in its times below)</span>
        </div>
        <div class="ctl">
          <div class="set-seg" role="radiogroup" aria-label="Profile">
            {#each PROFILES as p (p.id)}
              <button role="radio" aria-checked={form.profile === p.id} onclick={() => (form = withProfile(form!, p.id))}>{p.label}</button>
            {/each}
          </div>
        </div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">Freeze after</span><span class="help">Minutes in the background. 0 = never freeze.</span></div>
        <div class="ctl">
          <input class="lib-input narrow" type="number" min="0" max="1440" step="0.5" bind:value={form.freezeMin} disabled={!form.enabled} aria-label="Freeze after, minutes" aria-invalid={!!errors.freezeMin} /> min
        </div>
      </div>
      {#if errors.freezeMin}<p class="set-err pad">{errors.freezeMin}</p>{/if}
      <div class="set-row">
        <div class="text"><span class="label">Discard after</span><span class="help">Minutes in the background. 0 = never discard.</span></div>
        <div class="ctl">
          <input class="lib-input narrow" type="number" min="0" max="1440" step="1" bind:value={form.discardMin} disabled={!form.enabled} aria-label="Discard after, minutes" aria-invalid={!!errors.discardMin} /> min
        </div>
      </div>
      {#if errors.discardMin}<p class="set-err pad">{errors.discardMin}</p>{/if}
      <div class="set-row">
        <div class="text"><span class="label">Most live tabs</span><span class="help">Above this, the longest-idle tabs are discarded first. 0 = no limit.</span></div>
        <div class="ctl">
          <input class="lib-input narrow" type="number" min="0" max="500" step="1" bind:value={form.maxLiveTabs} disabled={!form.enabled} aria-label="Most live tabs" aria-invalid={!!errors.maxLiveTabs} />
        </div>
      </div>
      {#if errors.maxLiveTabs}<p class="set-err pad">{errors.maxLiveTabs}</p>{/if}
      {#if hasOverrides}
        <div class="set-actions">
          <button class="btn" disabled={saving} onclick={resetAll}>Reset to profile defaults</button>
          <span class="set-muted">Changed: {Object.entries(status.overrides).filter(([, v]) => v !== null && v !== undefined).map(([k]) => k.replace(/_/g, ' ')).join(', ')}</span>
        </div>
      {/if}
    </section>

    <section class="set-card" aria-labelledby="p-mem">
      <h3 id="p-mem"><span class="grow">Right now</span><span class="set-muted live" aria-hidden="true">updates every 3 s</span></h3>
      <div class="stats">
        <div><b>{mbLabel(status.memory.browser_mb)}</b><span>used by the browser</span></div>
        <div class:warn={status.memory.pressure}>
          <b>{mbLabel(status.memory.available_mb)}</b><span>free of {mbLabel(status.memory.total_mb)} ({status.memory.free_pct}%){status.memory.pressure ? ' · low memory' : ''}</span>
        </div>
        <div><b>{counts.live}</b><span>live tabs{status.limits.max_live_tabs ? ` (limit ${status.limits.max_live_tabs})` : ''}</span></div>
        <div><b>{counts.frozen} / {counts.discarded}</b><span>frozen / discarded</span></div>
      </div>
      <div class="set-table-wrap">
        <table class="set-table">
          <thead><tr><th>Tab</th><th>State</th><th class="r">Idle</th><th class="r">JS heap</th><th>Left alone because</th></tr></thead>
          <tbody>
            {#each status.tabs as t (t.id)}
              <tr>
                <td class="tab">
                  <Favicon url={t.url} />
                  <span class="t" title={t.url}>{t.title || t.url}</span>
                </td>
                <td><span class="lib-chip state-{t.state}">{t.state}</span></td>
                <td class="r">{t.idle_secs ? idleLabel(t.idle_secs) : 'active'}</td>
                <td class="r">{mbLabel(t.js_heap_mb)}</td>
                <td class="set-muted">{t.protected ?? '—'}</td>
              </tr>
            {:else}
              <tr><td colspan="5" class="set-muted">No tabs.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if status.decisions.length}
        <div class="dec">
          <h4>Recent decisions</h4>
          <ul>
            {#each status.decisions.slice(-6).reverse() as d, i (i)}
              <li>
                <span class="set-mono">{timeFmt.format(d.ts_ms)}</span>
                <span class="lib-chip">{d.action}</span>
                tab {d.tab}: {d.reason}{d.error ? ` — ${d.error}` : ''}
              </li>
            {/each}
          </ul>
        </div>
      {/if}
    </section>

    {#if dirty}
      <div class="set-savebar" role="region" aria-label="Unsaved changes">
        <span>{invalid ? 'Fix the highlighted values to save' : 'Unsaved changes'}</span>
        <button class="btn" disabled={saving} onclick={() => limits && (form = governorForm(limits))}>Discard</button>
        <button class="btn primary" disabled={saving || invalid} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
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
