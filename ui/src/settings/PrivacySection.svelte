<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { AdblockListUpdate, AdblockSettings, AdblockStats, SimplifyCheck, SimplifySiteArgs, SitePolicy, SitePolicyKey } from '../lib/api/types-settings';
  import Icon from '../lib/components/Icon.svelte';
  import { locale, t, tp } from '../lib/i18n/index.svelte';
  import { POLICY_KEYS, addSite, nextTriState, normalizeHost, policyRows, topCounts, type TriState } from '../lib/settings/forms';
  import Toggle from './Toggle.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  let adblock = $state<AdblockSettings | null>(null);
  let stats = $state<AdblockStats | null>(null);
  let sites = $state<Record<string, SitePolicy>>({});
  let loadError = $state<string | null>(null);
  let busy = $state(false);
  let updating = $state(false);
  let update = $state<AdblockListUpdate | null>(null);
  let allowInput = $state('');
  let allowError = $state<string | null>(null);
  let policyInput = $state('');
  let policyError = $state<string | null>(null);
  /** Hosts added in the table that have no stored field yet. */
  let pendingHosts = $state<string[]>([]);
  let checkInput = $state('');
  let check = $state<SimplifyCheck | null>(null);

  async function load() {
    try {
      const [list, st] = await Promise.all([api.call('simplify.list'), api.call('adblock.stats', {})]);
      adblock = list.adblock;
      sites = list.sites;
      stats = st;
      loadError = null;
    } catch (err) {
      loadError = (err as Error).message;
    }
  }
  $effect(() => {
    void load();
    const timer = setInterval(async () => {
      try {
        stats = await api.call('adblock.stats', {});
      } catch {
        // keep the last numbers
      }
    }, 5000);
    return () => clearInterval(timer);
  });

  async function run<T>(f: () => Promise<T>): Promise<T | undefined> {
    busy = true;
    try {
      return await f();
    } catch (err) {
      fail(err);
      return undefined;
    } finally {
      busy = false;
    }
  }

  const setGlobal = (on: boolean) =>
    run(async () => {
      adblock = await api.call('adblock.toggle', { on });
      notify(on ? t('settings.privacy.adblockOn') : t('settings.privacy.adblockOff'));
    });

  async function allowSite() {
    if (!adblock) return;
    const { error } = addSite(adblock.allowlist, allowInput);
    allowError = error;
    if (error) return;
    const host = normalizeHost(allowInput)!;
    await run(async () => {
      adblock = await api.call('adblock.toggle', { host, on: false });
      allowInput = '';
      notify(t('settings.privacy.adsAllowedOn', { host }));
    });
  }

  const blockAgain = (host: string) =>
    run(async () => {
      adblock = await api.call('adblock.toggle', { host, on: true });
    });

  async function updateLists(force = false) {
    updating = true;
    try {
      update = await api.call('adblock.update_lists', { force });
      const failed = update.lists.filter((l) => l.status === 'failed').length;
      notify(failed ? tp('settings.privacy.listsFailed', failed) : update.compiled ? t('settings.privacy.listsUpdated') : t('settings.privacy.listsCurrent'), failed > 0);
      void load();
    } catch (err) {
      fail(err);
    } finally {
      updating = false;
    }
  }

  const resetStats = () =>
    run(async () => {
      await api.call('adblock.stats', { reset: true });
      stats = await api.call('adblock.stats', {});
    });

  // ---- site policies ----
  const rows = $derived([...policyRows(sites), ...pendingHosts.filter((h) => !(h in sites)).map((host) => ({ host, policy: {} as SitePolicy }))]);

  function addPolicyHost() {
    const host = normalizeHost(policyInput);
    if (!host) {
      policyError = t('settings.privacy.err.policySite');
      return;
    }
    if (host in sites || pendingHosts.includes(host)) {
      policyError = t('settings.privacy.err.inTable', { host });
      return;
    }
    policyError = null;
    pendingHosts = [...pendingHosts, host];
    policyInput = '';
  }

  async function setPolicy(host: string, key: SitePolicyKey, value: TriState) {
    const current = { ...(sites[host] ?? {}) };
    let args: SimplifySiteArgs;
    if (value === null) {
      // Back to inherited: forget the stored policy and store the other fields again.
      delete current[key];
      args = { host, clear: true, ...current };
    } else {
      args = { host, [key]: value };
    }
    await run(async () => {
      const r = await api.call('simplify.site', args);
      const stored = Object.fromEntries(Object.entries(r.stored).filter(([, v]) => v !== null && v !== undefined)) as SitePolicy;
      if (Object.keys(stored).length) sites = { ...sites, [host]: stored };
      else {
        const { [host]: _gone, ...rest } = sites;
        sites = rest;
      }
    });
  }

  async function removePolicy(host: string) {
    pendingHosts = pendingHosts.filter((h) => h !== host);
    if (!(host in sites)) return;
    await run(async () => {
      await api.call('simplify.site', { host, clear: true });
      const { [host]: _gone, ...rest } = sites;
      sites = rest;
    });
  }

  async function checkSite() {
    const host = normalizeHost(checkInput);
    if (!host || host === '*') {
      check = null;
      return;
    }
    await run(async () => {
      check = await api.call('simplify.get', { host });
    });
  }

  const total = $derived(stats?.total ?? null);
  const pct = $derived(total && total.requests ? Math.round((total.blocked / total.requests) * 1000) / 10 : 0);
  const fmt = $derived(new Intl.NumberFormat(locale()));
  const label = (v: boolean | null | undefined) =>
    v === true ? t('settings.privacy.allowed') : v === false ? t('settings.privacy.blocked') : t('settings.privacy.inherited');
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.privacy.title')}</h2>
    <p>{t('settings.privacy.intro')}</p>
  </header>

  {#if loadError && !adblock}
    <p class="lib-error" role="alert">{t('settings.privacy.loadError', { error: loadError })}</p>
    <button class="btn" onclick={load}>{t('settings.hints.retry')}</button>
  {:else if !adblock}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="pr-ad">
      <h3 id="pr-ad">{t('settings.privacy.adblock')}</h3>
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.privacy.block')}</span>
          <span class="help">{t('settings.privacy.lists', { lists: adblock.lists.join(', ') || t('settings.privacy.none') })}{stats?.engine.loaded ? ` · ${t('settings.privacy.engine', { kb: Math.round(stats.engine.engine_bytes / 1024) })}` : ` · ${t('settings.privacy.notCompiled')}`}</span>
        </div>
        <div class="ctl"><Toggle label={t('settings.privacy.block')} checked={adblock.enabled} disabled={busy} onchange={setGlobal} /></div>
      </div>
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.privacy.filterLists')}</span>
          <span class="help">{t('settings.privacy.filterListsHelp')}</span>
        </div>
        <div class="ctl">
          <button class="btn" disabled={updating} onclick={() => updateLists(false)}>{updating ? t('settings.privacy.updating') : t('settings.privacy.updateNow')}</button>
          <button class="btn" disabled={updating} onclick={() => updateLists(true)} title={t('settings.privacy.forceHint')}>{t('settings.privacy.force')}</button>
        </div>
      </div>
      {#if update}
        <div class="set-table-wrap">
          <table class="set-table">
            <thead><tr><th>{t('settings.privacy.colList')}</th><th>{t('settings.privacy.colResult')}</th><th class="r">{t('settings.privacy.colSize')}</th></tr></thead>
            <tbody>
              {#each update.lists as l (l.id)}
                <tr class:bad={l.status === 'failed'}>
                  <td><span class="set-mono">{l.id}</span></td>
                  <td>
                    <span class="lib-chip" class:ok={l.status === 'updated'} class:bad={l.status === 'failed'}>{t(`settings.privacy.status.${l.status}`)}</span>
                    {#if l.error}<span class="set-err inline">{l.error}</span>{/if}
                  </td>
                  <td class="r">{t('settings.privacy.kb', { n: Math.round(l.bytes / 1024) })}</td>
                </tr>
              {/each}
            </tbody>
          </table>
          <p class="set-muted pad">{update.compiled ? t('settings.privacy.recompiled', { ms: update.compile_ms }) : t('settings.privacy.kept')}</p>
        </div>
      {/if}
      {#if total}
        <div class="stats">
          <div><b>{fmt.format(total.blocked)}</b><span>{t('settings.privacy.blockedOf', { n: fmt.format(total.requests), pct })}</span></div>
          <div><b>{fmt.format(total.by_filter)}</b><span>{t('settings.privacy.byFilter')}</span></div>
          <div><b>{fmt.format(total.by_policy)}</b><span>{t('settings.privacy.byPolicy')}</span></div>
        </div>
        <div class="tops">
          <div>
            <h4>{t('settings.privacy.mostBlocked')}</h4>
            {#each topCounts(total.blocked_hosts, 6) as h (h.key)}
              <div class="bar"><span class="set-mono">{h.key}</span><span class="r">{fmt.format(h.count)}</span></div>
            {:else}<p class="set-muted">{t('settings.privacy.nothingBlocked')}</p>{/each}
          </div>
          <div>
            <h4>{t('settings.privacy.byType')}</h4>
            {#each topCounts(total.blocked_by_type, 6) as h (h.key)}
              <div class="bar"><span>{h.key}</span><span class="r">{fmt.format(h.count)}</span></div>
            {:else}<p class="set-muted">—</p>{/each}
          </div>
        </div>
        <div class="set-actions"><button class="btn" disabled={busy} onclick={resetStats}>{t('settings.privacy.resetCounters')}</button></div>
      {/if}
    </section>

    <section class="set-card" aria-labelledby="pr-allow">
      <h3 id="pr-allow">{t('settings.privacy.allowTitle')}</h3>
      <p class="lead">{t('settings.privacy.allowLead')}</p>
      <div class="set-row stack">
        <div class="set-chips">
          {#each adblock.allowlist as host (host)}
            <span class="set-chip">
              {host}
              <button class="icon-btn" aria-label={t('settings.privacy.blockAgain', { host })} disabled={busy} onclick={() => blockAgain(host)}><Icon name="x" size={12} /></button>
            </span>
          {:else}
            <span class="set-muted">{t('settings.privacy.noAllowed')}</span>
          {/each}
        </div>
        <form class="add" onsubmit={(e) => (e.preventDefault(), void allowSite())}>
          <input class="lib-input wide" bind:value={allowInput} placeholder="example.com" spellcheck="false" aria-label={t('settings.privacy.allowInput')} aria-invalid={!!allowError} />
          <button class="btn" type="submit" disabled={busy || !allowInput.trim()}>{t('settings.privacy.allowAds')}</button>
        </form>
        {#if allowError}<p class="set-err">{allowError}</p>{/if}
      </div>
    </section>

    <section class="set-card" aria-labelledby="pr-pol">
      <h3 id="pr-pol">{t('settings.privacy.policies')}</h3>
      <p class="lead">
        {t('settings.privacy.policiesLead1')} <b>{t('settings.privacy.cycle')}</b>. {t('settings.privacy.policiesLead2')} <span class="set-mono">*</span>
        {t('settings.privacy.policiesLead3')}
      </p>
      <div class="set-table-wrap">
        <table class="set-table pol">
          <thead>
            <tr>
              <th>{t('settings.privacy.colSite')}</th>
              {#each POLICY_KEYS as k (k.key)}<th class="c">{k.label}</th>{/each}
              <th><span class="sr">{t('settings.privacy.remove')}</span></th>
            </tr>
          </thead>
          <tbody>
            {#each rows as row (row.host)}
              <tr>
                <td class="set-mono">{row.host === '*' ? t('settings.privacy.everySite') : row.host}</td>
                {#each POLICY_KEYS as k (k.key)}
                  {@const v = row.policy[k.key]}
                  <td class="c">
                    <button
                      class="cell"
                      class:allow={v === true}
                      class:block={v === false}
                      disabled={busy}
                      title={t('settings.privacy.cellHint', { what: k.label, host: row.host, state: label(v) })}
                      aria-label={t('settings.privacy.cell', { what: k.label, host: row.host, state: label(v) })}
                      onclick={() => setPolicy(row.host, k.key, nextTriState(v))}
                    >
                      {#if v === true}<Icon name="check" size={13} stroke={2.25} />{:else if v === false}<Icon name="x" size={13} stroke={2.25} />{:else}<span aria-hidden="true">·</span>{/if}
                    </button>
                  </td>
                {/each}
                <td class="r"><button class="icon-btn" aria-label={t('settings.privacy.removePolicy', { host: row.host })} disabled={busy} onclick={() => removePolicy(row.host)}><Icon name="trash" size={13} /></button></td>
              </tr>
            {:else}
              <tr><td colspan={POLICY_KEYS.length + 2} class="set-muted">{t('settings.privacy.noPolicies')}</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
      <form class="add pad2" onsubmit={(e) => (e.preventDefault(), addPolicyHost())}>
        <input class="lib-input wide" bind:value={policyInput} placeholder={t('settings.privacy.policyPlaceholder')} spellcheck="false" aria-label={t('settings.privacy.policyInput')} aria-invalid={!!policyError} />
        <button class="btn" type="submit" disabled={!policyInput.trim()}><Icon name="plus" size={13} /> {t('settings.privacy.addSite')}</button>
        {#if policyError}<p class="set-err">{policyError}</p>{/if}
      </form>
      <form class="add pad2 check" onsubmit={(e) => (e.preventDefault(), void checkSite())}>
        <input class="lib-input wide" bind:value={checkInput} placeholder={t('settings.privacy.checkPlaceholder')} spellcheck="false" aria-label={t('settings.privacy.checkInput')} />
        <button class="btn" type="submit" disabled={busy || !checkInput.trim()}>{t('settings.privacy.check')}</button>
        {#if check}
          <span class="set-muted" role="status">
            {check.host}: {check.adblock ? t('settings.privacy.adsBlocked') : t('settings.privacy.adsAllowed')} ·
            {t('settings.privacy.blockedList', { list: POLICY_KEYS.filter((k) => !check!.policy[k.key]).map((k) => k.label).join(', ') || t('settings.privacy.nothing') })}{check.matched.length ? ` ${t('settings.privacy.from', { list: check.matched.join(', ') })}` : ''}
          </span>
        {/if}
      </form>
    </section>
  {/if}
</div>

<style>
  .pad {
    padding: 6px 10px 10px;
  }
  .pad2 {
    padding: 10px 16px;
    border-top: 1px solid var(--border);
  }
  .check {
    padding-top: 0;
    border-top: 0;
  }
  .add {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }
  .inline {
    margin-left: 6px;
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 1px;
    margin: 0 16px 10px;
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
  .tops {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 24px;
    padding: 0 16px;
  }
  .tops h4 {
    margin: 4px 0 6px;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 650;
    letter-spacing: 0.03em;
    text-transform: uppercase;
  }
  .bar {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    padding: 3px 0;
    border-bottom: 1px solid var(--border);
    font-size: var(--fs-sm);
  }
  .bar .r {
    font-variant-numeric: tabular-nums;
  }
  .cell {
    display: inline-grid;
    place-items: center;
    width: 30px;
    height: 24px;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--fg-subtle);
  }
  .cell:hover:not(:disabled) {
    background: var(--hover);
  }
  .cell.allow {
    border-color: color-mix(in oklab, var(--ok) 40%, transparent);
    background: color-mix(in oklab, var(--ok) 12%, transparent);
    color: var(--ok);
  }
  .cell.block {
    border-color: color-mix(in oklab, var(--danger) 40%, transparent);
    background: color-mix(in oklab, var(--danger) 12%, transparent);
    color: var(--danger);
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  @media (max-width: 720px) {
    .tops,
    .stats {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>
