<script lang="ts">
  // Settings → Proxies: proxy pools (add, test, health, rotation), per-profile routing rules
  // (PAC), which profile uses what, and the local forwarders. proxy.* / geo.lookup
  // (crates/identity), profile.list (crates/profiles).
  import type { BrowserApi } from '../lib/api';
  import type { ProfileEntry } from '../lib/api/types';
  import type { GeoInfo, PoolStrategy, ProfileRoutes, ProxyForwarder, ProxyPool, ProxyRule, RulesTestResult } from '../lib/api/types-e2-settings';
  import Icon from '../lib/components/Icon.svelte';
  import { locale, t, tp } from '../lib/i18n/index.svelte';
  import {
    buildProxyUrl,
    emptyProxyForm,
    isMasked,
    parseProxyList,
    poolHealth,
    profileRouteRows,
    profilesUsingPool,
    proxyFormErrors,
    routeChoices,
    ruleErrors,
    type ProxyForm,
  } from '../lib/settings/e2-proxies';
  import Dialog from '../library/Dialog.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  const STRATEGIES: PoolStrategy[] = ['round_robin', 'random', 'sticky'];

  let pools = $state<ProxyPool[]>([]);
  let profiles = $state<ProfileEntry[]>([]);
  let routes = $state<ProfileRoutes[]>([]);
  let forwarders = $state<ProxyForwarder[]>([]);
  let loaded = $state(false);
  let loadError = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let expanded = $state<string | null>(null);

  // Add proxies.
  let target = $state('');
  let newPool = $state('');
  let mode = $state<'one' | 'list'>('one');
  let pf = $state<ProxyForm>(emptyProxyForm());
  let listText = $state('');
  let strategy = $state<PoolStrategy>('round_robin');
  let test = $state<{ ok: true; geo: GeoInfo; ms: number } | { ok: false; error: string } | null>(null);

  // Routing editor.
  let routing = $state<string | null>(null);
  let rules = $state<ProxyRule[]>([]);
  let routeDefault = $state('direct');
  let testUrl = $state('');
  let testResult = $state<RulesTestResult | null>(null);

  // Confirmation.
  let removing = $state<ProxyPool | null>(null);
  let removeOpen = $state(false);

  async function refresh() {
    try {
      const [pl, pr, rg, fw] = await Promise.all([
        api.call('proxy.pool_list'),
        api.call('profile.list'),
        api.call('proxy.rules_get', {}),
        api.call('proxy.forward_list'),
      ]);
      pools = pl.pools;
      profiles = pr.profiles;
      routes = rg.routes;
      forwarders = fw.forwarders;
      if (!target) target = pools[0]?.name ?? '__new';
      loadError = null;
      loaded = true;
    } catch (err) {
      loadError = (err as Error).message;
    }
  }

  $effect(() => {
    void refresh();
  });

  const formErrors = $derived(mode === 'one' ? proxyFormErrors(pf) : {});
  const listed = $derived(parseProxyList(listText));
  const poolName = $derived(target === '__new' ? newPool.trim() : target);
  const poolNameBad = $derived(target === '__new' && !!newPool && !/^[a-z0-9_-]{1,64}$/.test(newPool.trim()));
  const canAdd = $derived(
    !!poolName && !poolNameBad && (mode === 'one' ? Object.keys(formErrors).length === 0 : listed.length > 0),
  );
  const rows = $derived(profileRouteRows(profiles, routes));
  const errorsOfRules = $derived(ruleErrors(rules));
  const choices = $derived(routeChoices(pools, [routeDefault, ...rules.map((r) => r.via)]));

  async function addProxies() {
    if (!canAdd) return;
    busy = 'add';
    try {
      const proxies = mode === 'one' ? [buildProxyUrl(pf)] : listed;
      const isNew = !pools.some((p) => p.name === poolName);
      const r = await api.call('proxy.pool_add', { pool: poolName, proxies, ...(isNew ? { strategy } : {}) });
      notify(tp('settings.proxies.add.added', r.added, { pool: r.pool, total: r.total }));
      pf = emptyProxyForm();
      listText = '';
      newPool = '';
      test = null;
      target = r.pool;
      expanded = r.pool;
      await refresh();
    } catch (err) {
      fail(err);
    } finally {
      busy = null;
    }
  }

  async function testProxy() {
    if (Object.keys(formErrors).length) return;
    busy = 'test';
    test = null;
    const started = performance.now();
    try {
      const r = await api.call('geo.lookup', { proxy: buildProxyUrl(pf), timeout_ms: 10000 });
      test = { ok: true, geo: r.geo, ms: Math.round(performance.now() - started) };
    } catch (err) {
      test = { ok: false, error: (err as Error).message };
    } finally {
      busy = null;
    }
  }

  async function checkPool(pool: ProxyPool) {
    busy = `check:${pool.name}`;
    try {
      const r = await api.call('proxy.pool_check', { pool: pool.name });
      notify(t('settings.proxies.pool.checked', { pool: pool.name, healthy: r.healthy, unhealthy: r.unhealthy }));
      await refresh();
    } catch (err) {
      fail(err);
    } finally {
      busy = null;
    }
  }

  async function setStrategy(pool: ProxyPool, s: PoolStrategy) {
    busy = `strategy:${pool.name}`;
    try {
      await api.call('proxy.pool_add', { pool: pool.name, proxies: [], strategy: s });
      await refresh();
    } catch (err) {
      fail(err);
    } finally {
      busy = null;
    }
  }

  async function removeProxy(pool: ProxyPool, proxy: string) {
    busy = `rm:${proxy}`;
    try {
      await api.call('proxy.pool_remove', { pool: pool.name, proxy });
      await refresh();
    } catch (err) {
      fail(err);
    } finally {
      busy = null;
    }
  }

  async function removePool() {
    const pool = removing;
    removeOpen = false;
    if (!pool) return;
    busy = `rmpool:${pool.name}`;
    try {
      await api.call('proxy.pool_remove', { pool: pool.name });
      notify(t('settings.proxies.pool.deleted', { pool: pool.name }));
      await refresh();
    } catch (err) {
      fail(err);
    } finally {
      busy = null;
    }
  }

  function editRouting(id: string) {
    if (routing === id) {
      routing = null;
      return;
    }
    const r = routes.find((x) => x.profile === id);
    const p = profiles.find((x) => x.id === id);
    routing = id;
    rules = structuredClone($state.snapshot(r?.rules ?? []));
    routeDefault = r?.default ?? p?.proxy ?? 'direct';
    testUrl = '';
    testResult = null;
  }

  async function saveRouting() {
    if (!routing || Object.keys(errorsOfRules).length) return;
    busy = 'routes';
    try {
      // Masked URLs (user:***@) are sent back as shown: the core keeps their stored password.
      await api.call('proxy.rules_set', { profile: routing, rules: $state.snapshot(rules), default: routeDefault });
      notify(t('settings.proxies.routes.saved', { profile: routing }));
      await refresh();
    } catch (err) {
      fail(err);
    } finally {
      busy = null;
    }
  }

  async function testRoute() {
    if (!routing || !testUrl.trim()) return;
    busy = 'rtest';
    try {
      testResult = await api.call('proxy.rules_test', { profile: routing, url: testUrl.trim() });
    } catch (err) {
      fail(err);
    } finally {
      busy = null;
    }
  }

  function moveRule(i: number, by: number) {
    const j = i + by;
    if (j < 0 || j >= rules.length) return;
    const next = [...rules];
    [next[i], next[j]] = [next[j], next[i]];
    rules = next;
  }

  const strategyLabel = (s: PoolStrategy) => t(`settings.proxies.strategy.${s}`);
  // In the UI language (not the system's), and again when it changes.
  const dateFmt = $derived(new Intl.DateTimeFormat(locale(), { dateStyle: 'short', timeStyle: 'short' }));
  const viaLabel = (via: string) =>
    via === 'direct' ? t('settings.proxies.via.direct') : via.startsWith('pool:') ? t('settings.proxies.via.pool', { name: via.slice(5) }) : via;
  /** "3 proxies · 2 working · 1 down · median 184 ms · checked …" with a color per part. */
  const healthParts = (h: ReturnType<typeof poolHealth>) =>
    [
      { text: tp('settings.proxies.pool.count', h.total), cls: '' },
      h.healthy ? { text: t('settings.proxies.pool.healthy', { n: h.healthy }), cls: 'okc' } : null,
      h.unhealthy ? { text: t('settings.proxies.pool.unhealthy', { n: h.unhealthy }), cls: 'badc' } : null,
      h.unchecked ? { text: t('settings.proxies.pool.unchecked', { n: h.unchecked }), cls: '' } : null,
      h.medianMs !== null ? { text: t('settings.proxies.pool.median', { ms: h.medianMs }), cls: '' } : null,
      h.lastCheck ? { text: t('settings.proxies.pool.lastCheck', { when: dateFmt.format(h.lastCheck * 1000) }), cls: '' } : null,
    ].filter((p): p is { text: string; cls: string } => p !== null);
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.proxies.title')}</h2>
    <p>{t('settings.proxies.intro')}</p>
  </header>

  {#if loadError && !loaded}
    <p class="lib-error" role="alert">{t('settings.proxies.loadError', { error: loadError })}</p>
    <button class="btn" onclick={refresh}>{t('settings.proxies.retry')}</button>
  {:else if !loaded}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="x-pools">
      <h3 id="x-pools">{t('settings.proxies.pools.title')}</h3>
      <p class="lead">{t('settings.proxies.pools.lead')}</p>
      {#each pools as pool (pool.name)}
        {@const h = poolHealth(pool.proxies)}
        {@const users = profilesUsingPool(pool.name, profiles, routes)}
        <div class="set-row">
          <div class="text">
            <span class="label set-mono">pool:{pool.name}</span>
            <span class="help">
              {#each healthParts(h) as part, i (i)}{#if i}{' · '}{/if}<span class={part.cls}>{part.text}</span>{/each}
            </span>
            <span class="help">{users.length ? t('settings.proxies.pool.usedBy', { profiles: users.join(', ') }) : t('settings.proxies.pool.unused')}</span>
          </div>
          <div class="ctl">
            <select
              class="lib-input"
              value={pool.strategy}
              disabled={busy !== null}
              aria-label={t('settings.proxies.pool.strategyLabel', { pool: pool.name })}
              title={t(`settings.proxies.strategy.${pool.strategy}Help`)}
              onchange={(e) => setStrategy(pool, e.currentTarget.value as PoolStrategy)}
            >
              {#each STRATEGIES as s (s)}<option value={s}>{strategyLabel(s)}</option>{/each}
            </select>
            <button class="btn" disabled={busy !== null || !h.total} onclick={() => checkPool(pool)}>
              <Icon name="reload" size={13} />
              {busy === `check:${pool.name}` ? t('settings.proxies.pool.checking') : t('settings.proxies.pool.check')}
            </button>
            <button class="btn" aria-expanded={expanded === pool.name} onclick={() => (expanded = expanded === pool.name ? null : pool.name)}>
              {expanded === pool.name ? t('settings.proxies.pool.hide') : t('settings.proxies.pool.show')}
            </button>
            <button class="icon-btn" aria-label={t('settings.proxies.pool.delete', { pool: pool.name })} title={t('common.delete')} disabled={busy !== null} onclick={() => ((removing = pool), (removeOpen = true))}>
              <Icon name="trash" size={14} />
            </button>
          </div>
        </div>
        {#if expanded === pool.name}
          <div class="set-table-wrap">
            <table class="set-table">
              <thead>
                <tr>
                  <th>{t('settings.proxies.table.proxy')}</th>
                  <th>{t('settings.proxies.table.health')}</th>
                  <th class="r">{t('settings.proxies.table.latency')}</th>
                  <th>{t('settings.proxies.table.checked')}</th>
                  <th></th>
                </tr>
              </thead>
              <tbody>
                {#each pool.proxies as px (px.proxy)}
                  <tr class:bad={px.healthy === false}>
                    <td class="set-mono">{px.proxy}</td>
                    <td>
                      {#if px.healthy === true}<span class="lib-chip ok">{t('settings.proxies.table.ok')}</span>
                      {:else if px.healthy === false}<span class="lib-chip bad" title={px.error ?? ''}>{t('settings.proxies.table.down')}</span>
                      {:else}<span class="lib-chip">{t('settings.proxies.table.never')}</span>{/if}
                      {#if px.error}<span class="set-muted err">{px.error}</span>{/if}
                    </td>
                    <td class="r">{px.latency_ms !== null ? `${px.latency_ms} ms` : '—'}</td>
                    <td class="set-muted">{px.checked_at ? dateFmt.format(px.checked_at * 1000) : '—'}</td>
                    <td class="r">
                      <button class="icon-btn" aria-label={t('settings.proxies.table.remove', { proxy: px.proxy })} title={t('common.delete')} disabled={busy !== null} onclick={() => removeProxy(pool, px.proxy)}>
                        <Icon name="x" size={14} />
                      </button>
                    </td>
                  </tr>
                {:else}
                  <tr><td colspan="5" class="set-muted">{t('settings.proxies.table.empty')}</td></tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {:else}
        <p class="lead">{t('settings.proxies.pools.empty')}</p>
      {/each}
    </section>

    <section class="set-card" aria-labelledby="x-add">
      <h3 id="x-add">{t('settings.proxies.add.title')}</h3>
      <p class="lead">{t('settings.proxies.add.lead')}</p>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.proxies.add.pool')}</span><span class="help">{t('settings.proxies.add.poolHelp')}</span></div>
        <div class="ctl">
          <select class="lib-input" bind:value={target} aria-label={t('settings.proxies.add.pool')}>
            {#each pools as p (p.name)}<option value={p.name}>{p.name}</option>{/each}
            <option value="__new">{t('settings.proxies.add.newPool')}</option>
          </select>
          {#if target === '__new'}
            <input class="lib-input mono" bind:value={newPool} placeholder="residential-de" aria-label={t('settings.proxies.add.newPoolName')} aria-invalid={poolNameBad} />
            <select class="lib-input" bind:value={strategy} aria-label={t('settings.proxies.add.strategy')}>
              {#each STRATEGIES as s (s)}<option value={s}>{strategyLabel(s)}</option>{/each}
            </select>
          {/if}
        </div>
      </div>
      {#if poolNameBad}<p class="set-err pad">{t('settings.proxies.add.poolBad')}</p>{/if}
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.proxies.add.how')}</span></div>
        <div class="ctl">
          <div class="set-seg" role="radiogroup" aria-label={t('settings.proxies.add.how')}>
            <button role="radio" aria-checked={mode === 'one'} onclick={() => (mode = 'one')}>{t('settings.proxies.add.one')}</button>
            <button role="radio" aria-checked={mode === 'list'} onclick={() => (mode = 'list')}>{t('settings.proxies.add.list')}</button>
          </div>
        </div>
      </div>
      {#if mode === 'one'}
        <div class="set-row">
          <div class="text"><span class="label">{t('settings.proxies.add.type')}</span><span class="help">{t('settings.proxies.add.typeHelp')}</span></div>
          <div class="ctl">
            <div class="set-seg" role="radiogroup" aria-label={t('settings.proxies.add.type')}>
              <button role="radio" aria-checked={pf.scheme === 'http'} onclick={() => (pf.scheme = 'http')}>HTTP</button>
              <button role="radio" aria-checked={pf.scheme === 'socks5'} onclick={() => (pf.scheme = 'socks5')}>SOCKS5</button>
            </div>
          </div>
        </div>
        <div class="set-row">
          <div class="text"><span class="label">{t('settings.proxies.add.server')}</span><span class="help">{t('settings.proxies.add.serverHelp')}</span></div>
          <div class="ctl">
            <input class="lib-input mono" bind:value={pf.host} placeholder="proxy.example.com" aria-label={t('settings.proxies.add.host')} aria-invalid={!!formErrors.host && !!pf.host} />
            <input class="lib-input narrow mono" inputmode="numeric" bind:value={pf.port} placeholder={pf.scheme === 'socks5' ? '1080' : '3128'} aria-label={t('settings.proxies.add.port')} aria-invalid={!!formErrors.port && !!pf.port} />
          </div>
        </div>
        {#if pf.host && formErrors.host}<p class="set-err pad">{t(formErrors.host)}</p>{/if}
        {#if pf.port && formErrors.port}<p class="set-err pad">{t(formErrors.port)}</p>{/if}
        <div class="set-row">
          <div class="text"><span class="label">{t('settings.proxies.add.login')}</span><span class="help">{t('settings.proxies.add.loginHelp')}</span></div>
          <div class="ctl">
            <input class="lib-input" bind:value={pf.user} placeholder={t('settings.proxies.add.user')} aria-label={t('settings.proxies.add.user')} autocomplete="off" />
            <input class="lib-input" type="password" bind:value={pf.pass} placeholder={t('settings.proxies.add.pass')} aria-label={t('settings.proxies.add.pass')} autocomplete="new-password" />
          </div>
        </div>
        {#if formErrors.user}<p class="set-err pad">{t(formErrors.user)}</p>{/if}
        {#if test}
          <p class="pad test" class:set-ok={test.ok} class:set-err={!test.ok} role="status">
            {#if test.ok}
              {t('settings.proxies.add.testOk', { ip: test.geo.ip, place: [test.geo.city, test.geo.country].filter(Boolean).join(', '), tz: test.geo.timezone, ms: test.ms })}
            {:else}
              {t('settings.proxies.add.testFail', { error: test.error })}
            {/if}
          </p>
        {/if}
      {:else}
        <div class="set-row stack">
          <div class="text"><span class="help">{t('settings.proxies.add.listHelp')}</span></div>
          <textarea class="set-code short" bind:value={listText} spellcheck="false" aria-label={t('settings.proxies.add.list')} placeholder={'socks5://user:pass@1.2.3.4:1080\nhttp://user:pass@5.6.7.8:3128\n9.9.9.9:8000:user:pass'}></textarea>
          <span class="set-muted">{tp('settings.proxies.add.listCount', listed.length)}</span>
        </div>
      {/if}
      <div class="set-actions">
        <button class="btn primary" disabled={busy !== null || !canAdd} onclick={addProxies}><Icon name="plus" size={13} /> {t('settings.proxies.add.button')}</button>
        {#if mode === 'one'}
          <button class="btn" disabled={busy !== null || Object.keys(formErrors).length > 0} onclick={testProxy}>
            <Icon name="globe" size={13} />
            {busy === 'test' ? t('settings.proxies.add.testing') : t('settings.proxies.add.test')}
          </button>
        {/if}
      </div>
    </section>

    <section class="set-card" aria-labelledby="x-routes">
      <h3 id="x-routes">{t('settings.proxies.routes.title')}</h3>
      <p class="lead">{t('settings.proxies.routes.lead')}</p>
      {#each rows as r (r.id)}
        <div class="set-row">
          <div class="text">
            <span class="label">{r.label} <span class="set-mono sub">{r.id}</span></span>
            <span class="help">
              {t('settings.proxies.routes.default', { via: viaLabel(r.via) })}{#if r.rules}{' · '}{tp('settings.proxies.routes.rules', r.rules)}{/if}
            </span>
          </div>
          <div class="ctl">
            <button class="btn" aria-expanded={routing === r.id} onclick={() => editRouting(r.id)}>
              <Icon name="pencil" size={13} />
              {routing === r.id ? t('common.close') : t('settings.proxies.routes.edit')}
            </button>
          </div>
        </div>
        {#if routing === r.id}
          <div class="editor">
            <div class="set-row">
              <div class="text"><span class="label">{t('settings.proxies.routes.defaultLabel')}</span><span class="help">{t('settings.proxies.routes.defaultHelp')}</span></div>
              <div class="ctl">
                <select class="lib-input" bind:value={routeDefault} aria-label={t('settings.proxies.routes.defaultLabel')}>
                  {#each choices as c (c)}<option value={c}>{viaLabel(c)}</option>{/each}
                </select>
              </div>
            </div>
            <div class="set-row stack">
              <div class="text"><span class="label">{t('settings.proxies.routes.rulesLabel')}</span><span class="help">{t('settings.proxies.routes.rulesHelp')}</span></div>
              {#each rules as rule, i (i)}
                <div class="rule">
                  <span class="n">{i + 1}</span>
                  <input class="lib-input mono" bind:value={rule.match} placeholder="*.example.com" aria-label={t('settings.proxies.routes.match', { n: i + 1 })} aria-invalid={!!errorsOfRules[i]} />
                  <span aria-hidden="true">→</span>
                  <select class="lib-input" bind:value={rule.via} aria-label={t('settings.proxies.routes.via', { n: i + 1 })}>
                    {#each choices as c (c)}<option value={c}>{viaLabel(c)}</option>{/each}
                  </select>
                  <button class="icon-btn" aria-label={t('settings.proxies.routes.up', { n: i + 1 })} disabled={i === 0} onclick={() => moveRule(i, -1)}>↑</button>
                  <button class="icon-btn" aria-label={t('settings.proxies.routes.down', { n: i + 1 })} disabled={i === rules.length - 1} onclick={() => moveRule(i, 1)}>↓</button>
                  <button class="icon-btn" aria-label={t('settings.proxies.routes.remove', { n: i + 1 })} onclick={() => (rules = rules.filter((_, j) => j !== i))}><Icon name="x" size={14} /></button>
                  {#if isMasked(rule.via)}<span class="set-muted kept">{t('settings.proxies.routes.kept')}</span>{/if}
                </div>
                {#if errorsOfRules[i]}<p class="set-err">{t(errorsOfRules[i])}</p>{/if}
              {/each}
              <div>
                <button class="btn" onclick={() => (rules = [...rules, { match: '', via: 'direct' }])}><Icon name="plus" size={13} /> {t('settings.proxies.routes.add')}</button>
              </div>
            </div>
            <div class="set-actions">
              <button class="btn primary" disabled={busy !== null || Object.keys(errorsOfRules).length > 0} onclick={saveRouting}>{t('common.save')}</button>
            </div>
            <div class="set-row">
              <div class="text"><span class="label">{t('settings.proxies.routes.test')}</span><span class="help">{t('settings.proxies.routes.testHelp')}</span></div>
              <div class="ctl">
                <input class="lib-input wide mono" bind:value={testUrl} placeholder="https://www.example.com/" aria-label={t('settings.proxies.routes.test')} onkeydown={(e) => e.key === 'Enter' && testRoute()} />
                <button class="btn" disabled={busy !== null || !testUrl.trim()} onclick={testRoute}>{t('common.test')}</button>
              </div>
            </div>
            {#if testResult}
              <p class="pad set-muted" role="status">
                {testResult.rule
                  ? t('settings.proxies.routes.testRule', { n: testResult.rule, match: testResult.matched ?? '', via: viaLabel(testResult.via) })
                  : t('settings.proxies.routes.testDefault', { via: viaLabel(testResult.via) })}
                {#if testResult.upstream}{' '}{t('settings.proxies.routes.testUpstream', { upstream: testResult.upstream })}{/if}
              </p>
            {/if}
          </div>
        {/if}
      {:else}
        <p class="lead">{t('settings.proxies.routes.empty')}</p>
      {/each}
    </section>

    <section class="set-card" aria-labelledby="x-fwd">
      <h3 id="x-fwd">{t('settings.proxies.fwd.title')}</h3>
      <p class="lead">{t('settings.proxies.fwd.lead')}</p>
      <div class="set-table-wrap">
        <table class="set-table">
          <thead><tr><th>{t('settings.proxies.fwd.id')}</th><th>{t('settings.proxies.fwd.local')}</th><th>{t('settings.proxies.fwd.upstream')}</th></tr></thead>
          <tbody>
            {#each forwarders as f (f.id)}
              <tr><td class="set-mono">{f.id}</td><td class="set-mono">{f.socks5}</td><td class="set-mono">{f.upstream}</td></tr>
            {:else}
              <tr><td colspan="3" class="set-muted">{t('settings.proxies.fwd.empty')}</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    </section>
  {/if}
</div>

<Dialog bind:open={removeOpen} title={t('settings.proxies.pool.deleteTitle')} onsubmit={removePool}>
  <p>{t('settings.proxies.pool.deleteBody', { pool: removing?.name ?? '', n: removing?.proxies.length ?? 0 })}</p>
  {#if removing && profilesUsingPool(removing.name, profiles, routes).length}
    <p class="set-err">{t('settings.proxies.pool.deleteUsed', { profiles: profilesUsingPool(removing.name, profiles, routes).join(', ') })}</p>
  {/if}
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (removeOpen = false)}>{t('common.cancel')}</button>
    <button type="submit" class="btn danger primary">{t('common.delete')}</button>
  {/snippet}
</Dialog>

<style>
  .pad {
    padding: 0 16px 10px;
  }
  /* Long choices (preset titles, proxy URLs) must not squeeze the label and help away. */
  .set-row > .text {
    min-width: min(240px, 45%);
  }
  .ctl select.lib-input {
    max-width: 300px;
  }
  .ctl .btn,
  .set-actions .btn,
  .rule + div .btn,
  .set-row.stack > div > .btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .okc {
    color: var(--ok);
  }
  .badc {
    color: var(--danger);
  }
  .err {
    margin-left: 6px;
  }
  .sub {
    margin-left: 6px;
    color: var(--fg-subtle);
    font-weight: 400;
  }
  .short {
    min-height: 110px;
  }
  .test {
    margin: 0;
  }
  .editor {
    padding: 0 0 6px 26px;
    border-top: 1px solid var(--border);
    background: color-mix(in oklab, var(--sunken) 45%, transparent);
  }
  .rule {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .rule .n {
    width: 18px;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    text-align: right;
  }
  .rule input {
    flex: 1;
    min-width: 160px;
  }
  .kept {
    flex-basis: 100%;
    padding-left: 24px;
  }
</style>
