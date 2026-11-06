<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { AdblockListUpdate, AdblockSettings, AdblockStats, SimplifyCheck, SimplifySiteArgs, SitePolicy, SitePolicyKey } from '../lib/api/types-settings';
  import Icon from '../lib/components/Icon.svelte';
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
      notify(on ? 'Ad blocking is on' : 'Ad blocking is off (site policies still apply)');
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
      notify(`Ads are allowed on ${host}`);
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
      notify(failed ? `${failed} list${failed === 1 ? '' : 's'} could not be downloaded` : update.compiled ? 'Filter lists updated' : 'Filter lists are up to date', failed > 0);
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
      policyError = 'Enter a site like news.example.com, or * for every site';
      return;
    }
    if (host in sites || pendingHosts.includes(host)) {
      policyError = `${host} is already in the table`;
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
  const fmt = new Intl.NumberFormat();
  const label = (v: boolean | null | undefined) => (v === true ? 'Allowed' : v === false ? 'Blocked' : 'Inherited');
</script>

<div class="set-page">
  <header>
    <h2>Privacy &amp; blocking</h2>
    <p>
      Filter lists block ads and trackers for every tab, agents included. Site policies go further per site: no JavaScript, no
      images, nothing from other sites — handy for heavy pages and for fast scraping.
    </p>
  </header>

  {#if loadError && !adblock}
    <p class="lib-error" role="alert">Could not read the blocking settings: {loadError}</p>
    <button class="btn" onclick={load}>Retry</button>
  {:else if !adblock}
    <p class="set-muted">Loading…</p>
  {:else}
    <section class="set-card" aria-labelledby="pr-ad">
      <h3 id="pr-ad">Ad and tracker blocking</h3>
      <div class="set-row">
        <div class="text">
          <span class="label">Block ads and trackers</span>
          <span class="help">Lists: {adblock.lists.join(', ') || 'none'}{stats?.engine.loaded ? ` · engine ${Math.round(stats.engine.engine_bytes / 1024)} kB` : ' · not compiled yet'}</span>
        </div>
        <div class="ctl"><Toggle label="Block ads and trackers" checked={adblock.enabled} disabled={busy} onchange={setGlobal} /></div>
      </div>
      <div class="set-row">
        <div class="text">
          <span class="label">Filter lists</span>
          <span class="help">Downloads only what changed since the last update (HTTP 304 otherwise).</span>
        </div>
        <div class="ctl">
          <button class="btn" disabled={updating} onclick={() => updateLists(false)}>{updating ? 'Updating…' : 'Update now'}</button>
          <button class="btn" disabled={updating} onclick={() => updateLists(true)} title="Download every list again">Force</button>
        </div>
      </div>
      {#if update}
        <div class="set-table-wrap">
          <table class="set-table">
            <thead><tr><th>List</th><th>Result</th><th class="r">Size</th></tr></thead>
            <tbody>
              {#each update.lists as l (l.id)}
                <tr class:bad={l.status === 'failed'}>
                  <td><span class="set-mono">{l.id}</span></td>
                  <td>
                    <span class="lib-chip" class:ok={l.status === 'updated'} class:bad={l.status === 'failed'}>{l.status.replace('_', ' ')}</span>
                    {#if l.error}<span class="set-err inline">{l.error}</span>{/if}
                  </td>
                  <td class="r">{Math.round(l.bytes / 1024)} kB</td>
                </tr>
              {/each}
            </tbody>
          </table>
          <p class="set-muted pad">{update.compiled ? `Recompiled in ${update.compile_ms} ms.` : 'Nothing changed, the engine was kept.'}</p>
        </div>
      {/if}
      {#if total}
        <div class="stats">
          <div><b>{fmt.format(total.blocked)}</b><span>blocked of {fmt.format(total.requests)} requests ({pct}%)</span></div>
          <div><b>{fmt.format(total.by_filter)}</b><span>by filter lists</span></div>
          <div><b>{fmt.format(total.by_policy)}</b><span>by site policies</span></div>
        </div>
        <div class="tops">
          <div>
            <h4>Most blocked</h4>
            {#each topCounts(total.blocked_hosts, 6) as h (h.key)}
              <div class="bar"><span class="set-mono">{h.key}</span><span class="r">{fmt.format(h.count)}</span></div>
            {:else}<p class="set-muted">Nothing blocked yet.</p>{/each}
          </div>
          <div>
            <h4>By type</h4>
            {#each topCounts(total.blocked_by_type, 6) as h (h.key)}
              <div class="bar"><span>{h.key}</span><span class="r">{fmt.format(h.count)}</span></div>
            {:else}<p class="set-muted">—</p>{/each}
          </div>
        </div>
        <div class="set-actions"><button class="btn" disabled={busy} onclick={resetStats}>Reset counters</button></div>
      {/if}
    </section>

    <section class="set-card" aria-labelledby="pr-allow">
      <h3 id="pr-allow">Sites where ads are allowed</h3>
      <p class="lead">Filter lists are not applied on these sites and their subdomains (site policies still are).</p>
      <div class="set-row stack">
        <div class="set-chips">
          {#each adblock.allowlist as host (host)}
            <span class="set-chip">
              {host}
              <button class="icon-btn" aria-label="Block ads on {host} again" disabled={busy} onclick={() => blockAgain(host)}><Icon name="x" size={12} /></button>
            </span>
          {:else}
            <span class="set-muted">None: ads are blocked everywhere.</span>
          {/each}
        </div>
        <form class="add" onsubmit={(e) => (e.preventDefault(), void allowSite())}>
          <input class="lib-input wide" bind:value={allowInput} placeholder="example.com" spellcheck="false" aria-label="Site to allow ads on" aria-invalid={!!allowError} />
          <button class="btn" type="submit" disabled={busy || !allowInput.trim()}>Allow ads</button>
        </form>
        {#if allowError}<p class="set-err">{allowError}</p>{/if}
      </div>
    </section>

    <section class="set-card" aria-labelledby="pr-pol">
      <h3 id="pr-pol">Site policies</h3>
      <p class="lead">
        Click a cell to cycle <b>inherited → blocked → allowed</b>. A site inherits from <span class="set-mono">*</span> and from
        its parent domains. Takes effect on the next page load.
      </p>
      <div class="set-table-wrap">
        <table class="set-table pol">
          <thead>
            <tr>
              <th>Site</th>
              {#each POLICY_KEYS as k (k.key)}<th class="c">{k.label}</th>{/each}
              <th><span class="sr">Remove</span></th>
            </tr>
          </thead>
          <tbody>
            {#each rows as row (row.host)}
              <tr>
                <td class="set-mono">{row.host === '*' ? '* (every site)' : row.host}</td>
                {#each POLICY_KEYS as k (k.key)}
                  {@const v = row.policy[k.key]}
                  <td class="c">
                    <button
                      class="cell"
                      class:allow={v === true}
                      class:block={v === false}
                      disabled={busy}
                      title="{k.label} on {row.host}: {label(v)}. Click to change."
                      aria-label="{k.label} on {row.host}: {label(v)}"
                      onclick={() => setPolicy(row.host, k.key, nextTriState(v))}
                    >
                      {#if v === true}<Icon name="check" size={13} stroke={2.25} />{:else if v === false}<Icon name="x" size={13} stroke={2.25} />{:else}<span aria-hidden="true">·</span>{/if}
                    </button>
                  </td>
                {/each}
                <td class="r"><button class="icon-btn" aria-label="Remove the policy of {row.host}" disabled={busy} onclick={() => removePolicy(row.host)}><Icon name="trash" size={13} /></button></td>
              </tr>
            {:else}
              <tr><td colspan={POLICY_KEYS.length + 2} class="set-muted">No site policies: every site may load everything.</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
      <form class="add pad2" onsubmit={(e) => (e.preventDefault(), addPolicyHost())}>
        <input class="lib-input wide" bind:value={policyInput} placeholder="news.example.com or *" spellcheck="false" aria-label="Site to add a policy for" aria-invalid={!!policyError} />
        <button class="btn" type="submit" disabled={!policyInput.trim()}><Icon name="plus" size={13} /> Add site</button>
        {#if policyError}<p class="set-err">{policyError}</p>{/if}
      </form>
      <form class="add pad2 check" onsubmit={(e) => (e.preventDefault(), void checkSite())}>
        <input class="lib-input wide" bind:value={checkInput} placeholder="Check a site: shop.example.com" spellcheck="false" aria-label="Site to check" />
        <button class="btn" type="submit" disabled={busy || !checkInput.trim()}>Check</button>
        {#if check}
          <span class="set-muted" role="status">
            {check.host}: ads {check.adblock ? 'blocked' : 'allowed'} ·
            {POLICY_KEYS.filter((k) => !check!.policy[k.key]).map((k) => k.label).join(', ') || 'nothing'} blocked{check.matched.length ? ` (from ${check.matched.join(', ')})` : ''}
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
