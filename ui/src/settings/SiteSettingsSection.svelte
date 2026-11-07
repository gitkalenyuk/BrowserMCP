<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import Favicon from '../lib/components/Favicon.svelte';
  import Icon from '../lib/components/Icon.svelte';
  import { t, tp } from '../lib/i18n/index.svelte';
  import { COMMON_KINDS, groupRules, siteOrigin } from '../lib/settings/e1-data';
  import type { PermissionKind, PermissionRule, SiteZoom } from '../lib/settings/e1-types';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  /** Every kind the core knows, the common ones first (crates/profiles PermissionKind). */
  const KINDS: PermissionKind[] = [
    ...COMMON_KINDS,
    'midi',
    'storage_access',
    'local_fonts',
    'idle_detection',
    'window_management',
    'file_system',
    'multiple_downloads',
    'pointer_lock',
    'keyboard_lock',
    'local_network',
    'sensors',
    'other',
  ];

  let rules = $state<PermissionRule[] | null>(null);
  let zoom = $state<SiteZoom[] | null>(null);
  let loadError = $state<string | null>(null);
  let busy = $state(false);
  let filter = $state('');
  let draft = $state<{ site: string; kind: PermissionKind; allow: boolean }>({ site: '', kind: 'camera', allow: true });

  async function load() {
    const [p, z] = await Promise.allSettled([api.call('permissions.list'), api.call('zoom.list')]);
    if (p.status === 'fulfilled') rules = p.value.rules;
    else loadError = p.reason instanceof Error ? p.reason.message : String(p.reason);
    zoom = z.status === 'fulfilled' ? z.value.sites : [];
  }

  $effect(() => {
    void load();
  });

  const sites = $derived(
    groupRules(rules ?? []).filter((s) => !filter.trim() || s.origin.toLowerCase().includes(filter.trim().toLowerCase())),
  );
  const draftOrigin = $derived(siteOrigin(draft.site));

  async function setRule(origin: string, kind: PermissionKind, allow: boolean | null) {
    busy = true;
    try {
      rules = (await api.call('permissions.set', { origin, kind, allow })).rules;
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  async function resetSite(origin: string, list: PermissionRule[]) {
    busy = true;
    try {
      for (const r of list) rules = (await api.call('permissions.set', { origin, kind: r.kind, allow: null })).rules;
      notify(t('settings.sites.siteReset', { site: siteName(origin) }));
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  async function addRule() {
    if (!draftOrigin) return;
    await setRule(draftOrigin, draft.kind, draft.allow);
    notify(t(draft.allow ? 'settings.sites.ruleAllowed' : 'settings.sites.ruleBlocked', { kind: kindName(draft.kind), site: siteName(draftOrigin) }));
    draft = { ...draft, site: '' };
  }

  async function resetZoom(host?: string) {
    busy = true;
    try {
      const { reset } = await api.call('zoom.reset', host ? { host } : {});
      zoom = (await api.call('zoom.list')).sites;
      notify(host ? t('settings.sites.zoomResetOne', { site: host }) : tp('settings.sites.zoomResetAll', reset.length));
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  const kindName = (k: PermissionKind) => t(`settings.sites.kind.${k}`);
  const siteName = (origin: string) => (origin === '*' ? t('settings.sites.allSites') : origin.replace(/^https:\/\//, ''));
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.sites.title')}</h2>
    <p>{t('settings.sites.intro')}</p>
  </header>

  {#if loadError && !rules}
    <p class="lib-error" role="alert">{t('settings.sites.loadError', { error: loadError })}</p>
    <button class="btn" onclick={load}>{t('settings.sites.retry')}</button>
  {:else if !rules}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="ss-perm">
      <h3 id="ss-perm"><span class="grow">{t('settings.sites.permissions')}</span></h3>
      <p class="lead">{t('settings.sites.permissionsLead')}</p>
      {#if rules.length > 4}
        <div class="set-row">
          <input class="lib-input wide" type="search" bind:value={filter} placeholder={t('settings.sites.filter')} aria-label={t('settings.sites.filter')} />
        </div>
      {/if}
      {#each sites as s (s.origin)}
        <div class="site">
          <div class="site-head">
            {#if s.origin === '*'}<Icon name="globe" size={16} />{:else}<Favicon url={s.origin} />{/if}
            <span class="name" title={s.origin}>{siteName(s.origin)}</span>
            {#if s.origin === '*'}<span class="set-muted">{t('settings.sites.allSitesHelp')}</span>{/if}
            <button class="btn small" disabled={busy} onclick={() => resetSite(s.origin, s.rules)}>{t('settings.sites.resetSite')}</button>
          </div>
          <ul class="rules">
            {#each s.rules as r (r.kind)}
              <li>
                <span class="kind">{kindName(r.kind)}</span>
                <select
                  class="lib-input"
                  value={r.allow ? 'allow' : 'block'}
                  disabled={busy}
                  aria-label={t('settings.sites.ruleFor', { kind: kindName(r.kind), site: siteName(s.origin) })}
                  onchange={(e) => setRule(s.origin, r.kind, e.currentTarget.value === 'allow')}
                >
                  <option value="allow">{t('settings.sites.allow')}</option>
                  <option value="block">{t('settings.sites.block')}</option>
                </select>
                <button
                  class="icon-btn"
                  disabled={busy}
                  aria-label={t('settings.sites.forget', { kind: kindName(r.kind), site: siteName(s.origin) })}
                  title={t('settings.sites.forgetTitle')}
                  onclick={() => setRule(s.origin, r.kind, null)}
                ><Icon name="x" size={14} /></button>
              </li>
            {/each}
          </ul>
        </div>
      {:else}
        <p class="set-muted pad">{rules.length ? t('settings.sites.noMatch') : t('settings.sites.empty')}</p>
      {/each}

      <div class="set-row add">
        <div class="text"><span class="label">{t('settings.sites.addTitle')}</span><span class="help">{t('settings.sites.addHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input" spellcheck="false" bind:value={draft.site} placeholder={t('settings.sites.sitePlaceholder')} aria-label={t('settings.sites.site')} aria-invalid={!!draft.site.trim() && !draftOrigin} />
          <select class="lib-input" bind:value={draft.kind} aria-label={t('settings.sites.permission')}>
            {#each KINDS as k (k)}<option value={k}>{kindName(k)}</option>{/each}
          </select>
          <select class="lib-input" value={draft.allow ? 'allow' : 'block'} onchange={(e) => (draft.allow = e.currentTarget.value === 'allow')} aria-label={t('settings.sites.answer')}>
            <option value="allow">{t('settings.sites.allow')}</option>
            <option value="block">{t('settings.sites.block')}</option>
          </select>
          <button class="btn" disabled={busy || !draftOrigin} onclick={addRule}>{t('common.add')}</button>
        </div>
      </div>
      {#if draft.site.trim() && !draftOrigin}<p class="set-err pad">{t('settings.sites.badSite')}</p>{/if}
    </section>

    <section class="set-card" aria-labelledby="ss-zoom">
      <h3 id="ss-zoom">{t('settings.sites.zoom')}</h3>
      <p class="lead">{t('settings.sites.zoomLead')}</p>
      {#if zoom && zoom.length}
        <div class="set-table-wrap">
          <table class="set-table">
            <thead><tr><th>{t('settings.sites.site')}</th><th class="r">{t('settings.sites.zoomLevel')}</th><th class="r"><span class="sr">{t('settings.sites.actions')}</span></th></tr></thead>
            <tbody>
              {#each zoom as z (z.host)}
                <tr>
                  <td class="host"><Favicon url={`https://${z.host}/`} /><span>{z.host}</span></td>
                  <td class="r">{Math.round(z.percent)} %</td>
                  <td class="r"><button class="btn small" disabled={busy} onclick={() => resetZoom(z.host)}>{t('settings.sites.zoomReset')}</button></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
        <div class="set-actions"><button class="btn" disabled={busy} onclick={() => resetZoom()}>{t('settings.sites.zoomResetAllButton')}</button></div>
      {:else}
        <p class="set-muted pad">{t('settings.sites.zoomEmpty')}</p>
      {/if}
    </section>

    <section class="set-card" aria-labelledby="ss-popups">
      <h3 id="ss-popups">{t('settings.sites.popups')}</h3>
      <p class="lead">{t('settings.sites.popupsLead')}</p>
    </section>
  {/if}
</div>

<style>
  .pad {
    padding: 0 16px 12px;
  }
  .site {
    border-top: 1px solid var(--border);
    padding: 8px 16px;
  }
  .site-head {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .site-head .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    font-weight: 550;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .site-head .set-muted {
    flex: 2;
  }
  .rules {
    display: flex;
    flex-wrap: wrap;
    gap: 6px 16px;
    margin: 6px 0 2px 26px;
    padding: 0;
    list-style: none;
  }
  .rules li {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .rules .kind {
    color: var(--fg-muted);
    font-size: var(--fs-sm);
  }
  .add .ctl {
    flex: 2;
  }
  .host {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .btn.small {
    height: 26px;
    padding: 0 10px;
    font-size: var(--fs-sm);
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
</style>
