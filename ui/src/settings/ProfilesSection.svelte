<script lang="ts">
  // Settings → Profiles & identity: profiles (= workspaces) with their proxy, language and
  // fingerprint identity. profile.* (crates/profiles), identity.* / stealth.check
  // (crates/identity), proxy.pool_list / proxy.forward_list for the proxy choices.
  import type { BrowserApi } from '../lib/api';
  import type { ProfileEntry } from '../lib/api/types';
  import type { IdentityPreset, IdentitySummary, ProxyForwarder, ProxyPool, StealthCheck } from '../lib/api/types-e2-settings';
  import Icon from '../lib/components/Icon.svelte';
  import { t, tp } from '../lib/i18n/index.svelte';
  import {
    fitProxyRef,
    identityIdFor,
    presetGroups,
    profileForm,
    profileIdError,
    profileIdFromName,
    profileProxyError,
    profileSave,
    webrtcLimited,
    type ProfileForm,
  } from '../lib/settings/e2-profiles';
  import { poolRef } from '../lib/settings/e2-proxies';
  import Dialog from '../library/Dialog.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  let profiles = $state<ProfileEntry[]>([]);
  let identities = $state<IdentitySummary[]>([]);
  let presets = $state<IdentityPreset[]>([]);
  let pools = $state<ProxyPool[]>([]);
  let forwarders = $state<ProxyForwarder[]>([]);
  let loaded = $state(false);
  let loadError = $state<string | null>(null);
  let busy = $state(false);

  // New profile.
  let newName = $state('');
  let newId = $state('');
  let idTouched = $state(false);

  // The profile being edited.
  let editing = $state<string | null>(null);
  let form = $state<ProfileForm | null>(null);
  let customProxy = $state(false);
  let presetPick = $state('');
  let fitResult = $state<{ profile: string; lines: string[] } | null>(null);
  let check = $state<{ profile: string; result: StealthCheck } | null>(null);

  // Confirmations.
  let deleting = $state<ProfileEntry | null>(null);
  let deleteOpen = $state(false);
  let keepData = $state(false);
  let deletingIdentity = $state<IdentitySummary | null>(null);
  let identityOpen = $state(false);

  async function refresh() {
    try {
      const [p, i, pr, pl, fw] = await Promise.all([
        api.call('profile.list'),
        api.call('identity.list'),
        api.call('identity.presets'),
        api.call('proxy.pool_list'),
        api.call('proxy.forward_list'),
      ]);
      profiles = p.profiles;
      identities = i.identities;
      presets = pr.presets;
      pools = pl.pools;
      forwarders = fw.forwarders;
      loadError = null;
      loaded = true;
    } catch (err) {
      loadError = (err as Error).message;
    }
  }

  $effect(() => {
    void refresh();
  });

  $effect(() => {
    if (!idTouched) newId = profileIdFromName(newName);
  });

  const newIdError = $derived(
    newName || newId
      ? profileIdError(
          newId,
          profiles.map((p) => p.id),
        )
      : '',
  );
  const current = $derived(profiles.find((p) => p.id === editing) ?? null);
  const save = $derived(current && form ? profileSave(current, form) : {});
  const dirty = $derived(save.label !== undefined || !!save.create);
  const proxyError = $derived(current && form ? profileProxyError(form.proxy, current.proxy ?? '') : '');
  const groups = $derived(presetGroups(presets));
  const usedBy = (id: string) => profiles.filter((p) => p.identity === id).map((p) => p.label);
  const identityOf = (p: ProfileEntry) => identities.find((i) => i.id === p.identity) ?? null;
  const proxyChoices = $derived(pools.map((p) => poolRef(p.name)));

  function edit(p: ProfileEntry) {
    if (editing === p.id) {
      editing = null;
      return;
    }
    editing = p.id;
    form = profileForm(p);
    customProxy = !!form.proxy && !proxyChoices.includes(form.proxy);
    presetPick = '';
    fitResult = null;
    check = null;
  }

  async function create() {
    if (newIdError || !newId) return;
    busy = true;
    try {
      const r = await api.call('profile.create', { id: newId });
      if (newName.trim() && newName.trim() !== newId) await api.call('profile.update', { id: newId, label: newName.trim() });
      notify(r.created ? t('settings.profiles.created', { name: newName.trim() || newId }) : t('settings.profiles.exists', { id: newId }));
      newName = '';
      newId = '';
      idTouched = false;
      await refresh();
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  async function saveProfile() {
    if (!current || !form || proxyError) return;
    busy = true;
    try {
      if (save.label !== undefined) await api.call('profile.update', { id: current.id, label: save.label });
      if (save.create) {
        const r = await api.call('profile.create', { id: current.id, ...save.create });
        if (r.warning) notify(r.warning, true);
      }
      notify(t('settings.profiles.saved', { name: form.label || current.id }));
      await refresh();
      const again = profiles.find((p) => p.id === current.id);
      if (again) form = profileForm(again);
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  async function assign(p: ProfileEntry, id: string) {
    busy = true;
    try {
      await api.call('identity.assign', id ? { profile: p.id, id } : { profile: p.id });
      notify(id ? t('settings.profiles.identity.assigned', { id, name: p.label }) : t('settings.profiles.identity.unassigned', { name: p.label }));
      await refresh();
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  async function createIdentity(p: ProfileEntry) {
    if (!presetPick) return;
    busy = true;
    try {
      const id = identityIdFor(p.id, presetPick);
      await api.call('identity.generate', { preset: presetPick, id, save: true });
      await api.call('identity.assign', { profile: p.id, id });
      notify(t('settings.profiles.identity.createdFor', { id, name: p.label }));
      presetPick = '';
      await refresh();
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  async function fit(p: ProfileEntry) {
    const ref = fitProxyRef(p, forwarders);
    if (!p.identity || !ref) return;
    busy = true;
    try {
      const r = await api.call('identity.fit_to_proxy', { identity_id: p.identity, proxy_id: ref, profile: p.id });
      const place = [r.geo.city, r.geo.country].filter(Boolean).join(', ');
      const lines = [t('settings.profiles.fit.where', { ip: r.geo.ip, place, tz: r.geo.timezone })];
      if (!r.changes.length) lines.push(t('settings.profiles.fit.nothing'));
      for (const c of r.changes) lines.push(`${c.field}: ${JSON.stringify(c.from)} → ${JSON.stringify(c.to)}`);
      fitResult = { profile: p.id, lines };
      await refresh();
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  async function runCheck(p: ProfileEntry) {
    const tab = p.tabs[0];
    if (tab === undefined) return;
    busy = true;
    try {
      check = { profile: p.id, result: await api.call('stealth.check', { tab }) };
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  async function removeProfile() {
    const p = deleting;
    deleteOpen = false;
    if (!p) return;
    busy = true;
    try {
      await api.call('profile.delete', { id: p.id, keep_data: keepData });
      notify(t('settings.profiles.deleted', { name: p.label }));
      if (editing === p.id) editing = null;
      await refresh();
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  async function removeIdentity() {
    const i = deletingIdentity;
    identityOpen = false;
    if (!i) return;
    busy = true;
    try {
      await api.call('identity.remove', { id: i.id });
      notify(t('settings.profiles.identity.deleted', { id: i.id }));
      await refresh();
    } catch (err) {
      fail(err);
    } finally {
      busy = false;
    }
  }

  /** Why a profile cannot be deleted ('' = it can). */
  const cannotDelete = (p: ProfileEntry) =>
    p.launch ? t('settings.profiles.delete.launch') : p.tabs.length ? t('settings.profiles.delete.tabs') : '';
  const osLabel = (os: string) => t(`settings.profiles.os.${os}`);
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.profiles.title')}</h2>
    <p>{t('settings.profiles.intro')}</p>
  </header>

  {#if loadError && !loaded}
    <p class="lib-error" role="alert">{t('settings.profiles.loadError', { error: loadError })}</p>
    <button class="btn" onclick={refresh}>{t('settings.profiles.retry')}</button>
  {:else if !loaded}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="p-new">
      <h3 id="p-new">{t('settings.profiles.new.title')}</h3>
      <p class="lead">{t('settings.profiles.new.lead')}</p>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.profiles.new.name')}</span><span class="help">{t('settings.profiles.new.nameHelp')}</span></div>
        <div class="ctl"><input class="lib-input wide" bind:value={newName} placeholder={t('settings.profiles.new.namePlaceholder')} aria-label={t('settings.profiles.new.name')} /></div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.profiles.new.id')}</span><span class="help">{t('settings.profiles.new.idHelp')}</span></div>
        <div class="ctl">
          <input
            class="lib-input mono"
            bind:value={newId}
            oninput={() => (idTouched = true)}
            aria-label={t('settings.profiles.new.id')}
            aria-invalid={!!newIdError}
          />
          <button class="btn primary" disabled={busy || !newId || !!newIdError} onclick={create}><Icon name="plus" size={13} /> {t('settings.profiles.new.create')}</button>
        </div>
      </div>
      {#if newIdError}<p class="set-err pad">{t(newIdError)}</p>{/if}
    </section>

    <section class="set-card" aria-labelledby="p-list">
      <h3 id="p-list">{t('settings.profiles.list.title')}</h3>
      <p class="lead">{t('settings.profiles.list.lead')}</p>
      {#each profiles as p (p.id + (p.incognito ? ':mem' : ''))}
        {@const ident = identityOf(p)}
        <div class="set-row prof" class:open={editing === p.id}>
          <span class="dot" style:--c={p.color ? `var(--g-${p.color})` : 'var(--fg-subtle)'} aria-hidden="true"></span>
          <div class="text">
            <span class="label">{p.label}{#if p.label !== p.id}<span class="set-mono id">{p.id}</span>{/if}</span>
            <span class="chips">
              {#if p.launch}<span class="lib-chip">{t('settings.profiles.chip.launch')}</span>{/if}
              {#if p.incognito}<span class="lib-chip warn">{t('settings.profiles.chip.memory')}</span>{/if}
              {#if p.tabs.length}<span class="lib-chip ok">{tp('settings.profiles.chip.tabs', p.tabs.length)}</span>{/if}
              <span class="lib-chip">{p.proxy ? t('settings.profiles.chip.proxy', { proxy: p.proxy }) : t('settings.profiles.chip.direct')}</span>
              {#if p.identity}
                <span class="lib-chip" class:warn={ident?.kind === 'device_emulation'}>
                  {ident?.kind === 'device_emulation' ? t('settings.profiles.chip.emulation', { id: p.identity }) : t('settings.profiles.chip.identity', { id: p.identity })}
                </span>
              {/if}
              {#if p.accept_language}<span class="lib-chip">{p.accept_language}</span>{/if}
            </span>
          </div>
          <div class="ctl">
            {#if !p.incognito}
              <button class="btn" aria-expanded={editing === p.id} onclick={() => edit(p)}><Icon name="pencil" size={13} /> {editing === p.id ? t('common.close') : t('common.edit')}</button>
            {/if}
            <button
              class="icon-btn"
              aria-label={t('settings.profiles.delete.label', { name: p.label })}
              title={cannotDelete(p) || t('common.delete')}
              disabled={busy || !!cannotDelete(p) || p.incognito}
              onclick={() => ((deleting = p), (keepData = false), (deleteOpen = true))}><Icon name="trash" size={14} /></button
            >
          </div>
        </div>

        {#if editing === p.id && form}
          <div class="editor">
            <div class="set-row">
              <div class="text"><span class="label">{t('settings.profiles.edit.name')}</span><span class="help">{t('settings.profiles.edit.nameHelp')}</span></div>
              <div class="ctl"><input class="lib-input wide" bind:value={form.label} placeholder={p.id} aria-label={t('settings.profiles.edit.name')} /></div>
            </div>
            <div class="set-row">
              <div class="text">
                <span class="label">{t('settings.profiles.edit.proxy')}</span>
                <span class="help">{t('settings.profiles.edit.proxyHelp')}</span>
              </div>
              <div class="ctl">
                <select
                  class="lib-input"
                  aria-label={t('settings.profiles.edit.proxy')}
                  value={customProxy ? '__custom' : form.proxy}
                  onchange={(e) => {
                    const v = e.currentTarget.value;
                    customProxy = v === '__custom';
                    if (!customProxy) form!.proxy = v;
                  }}
                >
                  <option value="">{t('settings.profiles.edit.direct')}</option>
                  {#each proxyChoices as ref (ref)}<option value={ref}>{t('settings.profiles.edit.pool', { name: ref.slice(5) })}</option>{/each}
                  <option value="__custom">{t('settings.profiles.edit.custom')}</option>
                </select>
              </div>
            </div>
            {#if customProxy}
              <div class="set-row">
                <div class="text"><span class="help">{t('settings.profiles.edit.customHelp')}</span></div>
                <div class="ctl">
                  <input class="lib-input wide mono" bind:value={form.proxy} placeholder="socks5://user:pass@host:1080" aria-label={t('settings.profiles.edit.customLabel')} aria-invalid={!!proxyError} />
                </div>
              </div>
            {/if}
            {#if proxyError}<p class="set-err pad">{t(proxyError)}</p>{/if}
            <div class="set-row">
              <div class="text"><span class="label">{t('settings.profiles.edit.webrtc')}</span><span class="help">{t('settings.profiles.edit.webrtcHelp')}</span></div>
              <div class="ctl">
                <span class="lib-chip" class:ok={webrtcLimited(p)}>{webrtcLimited(p) ? t('settings.profiles.edit.webrtcLimited') : t('settings.profiles.edit.webrtcNormal')}</span>
              </div>
            </div>
            <div class="set-row">
              <div class="text"><span class="label">{t('settings.profiles.edit.lang')}</span><span class="help">{t('settings.profiles.edit.langHelp')}</span></div>
              <div class="ctl"><input class="lib-input wide mono" bind:value={form.acceptLanguage} placeholder="uk-UA,uk,en-US,en" aria-label={t('settings.profiles.edit.lang')} /></div>
            </div>
            {#if dirty}
              <div class="set-actions">
                <button class="btn primary" disabled={busy || !!proxyError} onclick={saveProfile}>{t('common.save')}</button>
                <button class="btn" disabled={busy} onclick={() => ((form = profileForm(p)), (customProxy = false))}>{t('settings.profiles.edit.discard')}</button>
              </div>
            {/if}

            <h4>{t('settings.profiles.identity.title')}</h4>
            <p class="sub">{t('settings.profiles.identity.lead')}</p>
            <div class="set-row">
              <div class="text">
                <span class="label">{t('settings.profiles.identity.use')}</span>
                {#if ident}
                  <span class="help">{t('settings.profiles.identity.summary', { os: osLabel(ident.os), w: ident.screen.width, h: ident.screen.height, tz: ident.timezone, locale: ident.locale })}</span>
                  <span class="help set-mono ua">{ident.user_agent}</span>
                  {#if ident.kind === 'device_emulation'}<span class="help warnc">{t('settings.profiles.identity.emulationNote')}</span>{/if}
                {:else}
                  <span class="help">{t('settings.profiles.identity.noneHelp')}</span>
                {/if}
              </div>
              <div class="ctl">
                <select class="lib-input" aria-label={t('settings.profiles.identity.use')} value={p.identity ?? ''} disabled={busy} onchange={(e) => assign(p, e.currentTarget.value)}>
                  <option value="">{t('settings.profiles.identity.none')}</option>
                  {#each identities as i (i.id)}<option value={i.id}>{i.id} · {i.preset}</option>{/each}
                </select>
              </div>
            </div>
            <div class="set-row">
              <div class="text"><span class="label">{t('settings.profiles.identity.fromPreset')}</span><span class="help">{t('settings.profiles.identity.fromPresetHelp')}</span></div>
              <div class="ctl">
                <select class="lib-input" bind:value={presetPick} aria-label={t('settings.profiles.identity.fromPreset')}>
                  <option value="">{t('settings.profiles.identity.pickPreset')}</option>
                  {#each groups as g (g.group)}
                    <optgroup label={t(`settings.profiles.preset.${g.group}`)}>
                      {#each g.presets as pr (pr.name)}<option value={pr.name}>{pr.title}</option>{/each}
                    </optgroup>
                  {/each}
                </select>
                <button class="btn" disabled={busy || !presetPick} onclick={() => createIdentity(p)}>{t('settings.profiles.identity.create')}</button>
              </div>
            </div>
            <div class="set-row">
              <div class="text">
                <span class="label">{t('settings.profiles.fit.label')}</span>
                <span class="help">{t('settings.profiles.fit.help')}</span>
                {#if p.identity && !fitProxyRef(p, forwarders)}<span class="help warnc">{t('settings.profiles.fit.cannot')}</span>{/if}
              </div>
              <div class="ctl">
                <button class="btn" disabled={busy || !p.identity || !fitProxyRef(p, forwarders)} onclick={() => fit(p)}><Icon name="globe" size={13} /> {t('settings.profiles.fit.button')}</button>
              </div>
            </div>
            {#if fitResult?.profile === p.id}
              <ul class="result" role="status">{#each fitResult.lines as line, i (i)}<li>{line}</li>{/each}</ul>
            {/if}
            <div class="set-row">
              <div class="text">
                <span class="label">{t('settings.profiles.check.label')}</span>
                <span class="help">{p.tabs.length ? t('settings.profiles.check.help') : t('settings.profiles.check.noTabs')}</span>
              </div>
              <div class="ctl">
                <button class="btn" disabled={busy || !p.tabs.length} onclick={() => runCheck(p)}><Icon name="eye" size={13} /> {t('settings.profiles.check.button')}</button>
              </div>
            </div>
            {#if check?.profile === p.id}
              {@const r = check.result}
              <div class="result" role="status">
                <p>{t('settings.profiles.check.summary', { passed: r.passed, warned: r.warned, failed: r.failed })}</p>
                <ul>
                  {#each r.checks as c (c.name)}
                    <li><span class="lib-chip" class:ok={c.status === 'pass'} class:warn={c.status === 'warn'} class:bad={c.status === 'fail'}>{c.status}</span> {c.name} <span class="set-muted">{c.detail}</span></li>
                  {/each}
                </ul>
              </div>
            {/if}
          </div>
        {/if}
      {:else}
        <p class="lead">{t('settings.profiles.list.empty')}</p>
      {/each}
    </section>

    <section class="set-card" aria-labelledby="p-ids">
      <h3 id="p-ids">{t('settings.profiles.ids.title')}</h3>
      <p class="lead">{t('settings.profiles.ids.lead')}</p>
      <div class="set-table-wrap">
        <table class="set-table">
          <thead>
            <tr>
              <th>{t('settings.profiles.ids.id')}</th>
              <th>{t('settings.profiles.ids.device')}</th>
              <th>{t('settings.profiles.ids.place')}</th>
              <th>{t('settings.profiles.ids.usedBy')}</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {#each identities as i (i.id)}
              <tr>
                <td class="set-mono">{i.id}</td>
                <td>
                  {osLabel(i.os)} · {i.screen.width}×{i.screen.height}
                  {#if i.kind === 'device_emulation'}<span class="lib-chip warn">{t('settings.profiles.ids.emulation')}</span>{/if}
                </td>
                <td>{i.timezone} · {i.locale}</td>
                <td class="set-muted">{usedBy(i.id).join(', ') || '—'}</td>
                <td class="r">
                  <button class="icon-btn" aria-label={t('settings.profiles.ids.delete', { id: i.id })} title={t('common.delete')} disabled={busy} onclick={() => ((deletingIdentity = i), (identityOpen = true))}><Icon name="trash" size={14} /></button>
                </td>
              </tr>
            {:else}
              <tr><td colspan="5" class="set-muted">{t('settings.profiles.ids.empty')}</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    </section>
  {/if}
</div>

<Dialog bind:open={deleteOpen} title={t('settings.profiles.delete.title')} onsubmit={removeProfile}>
  <p>{t('settings.profiles.delete.body', { name: deleting?.label ?? '', id: deleting?.id ?? '' })}</p>
  <label class="keep"><input type="checkbox" bind:checked={keepData} /> {t('settings.profiles.delete.keep')}</label>
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (deleteOpen = false)}>{t('common.cancel')}</button>
    <button type="submit" class="btn danger primary">{t('common.delete')}</button>
  {/snippet}
</Dialog>

<Dialog bind:open={identityOpen} title={t('settings.profiles.ids.deleteTitle')} onsubmit={removeIdentity}>
  <p>{t('settings.profiles.ids.deleteBody', { id: deletingIdentity?.id ?? '' })}</p>
  {#if deletingIdentity && usedBy(deletingIdentity.id).length}
    <p class="set-muted">{t('settings.profiles.ids.deleteUsed', { profiles: usedBy(deletingIdentity.id).join(', ') })}</p>
  {/if}
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (identityOpen = false)}>{t('common.cancel')}</button>
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
  .ctl .btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .dot {
    flex: none;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--c);
  }
  .id {
    margin-left: 6px;
    color: var(--fg-subtle);
    font-weight: 400;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 2px;
  }
  .prof.open {
    background: var(--accent-soft);
  }
  .editor {
    padding: 0 0 6px 26px;
    border-top: 1px solid var(--border);
    background: color-mix(in oklab, var(--sunken) 45%, transparent);
  }
  .editor h4 {
    margin: 0;
    padding: 12px 16px 2px;
    border-top: 1px solid var(--border);
    font-size: var(--fs-sm);
    font-weight: 650;
  }
  .sub {
    margin: 0;
    padding: 0 16px 8px;
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
    line-height: 1.45;
  }
  .ua {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .warnc {
    color: var(--warn);
  }
  .result {
    margin: 0 16px 10px;
    padding: 8px 12px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
    font-size: var(--fs-sm);
    line-height: 1.5;
    list-style: none;
  }
  .result p {
    margin: 0 0 4px;
    font-weight: 600;
  }
  .result ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .keep {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--fs-sm);
  }
</style>
