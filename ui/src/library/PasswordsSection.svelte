<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { PasswordEntry } from '../lib/api/types-data';
  import Favicon from '../lib/components/Favicon.svelte';
  import Icon from '../lib/components/Icon.svelte';
  import { t } from '../lib/i18n/index.svelte';
  import { editPassword } from '../lib/data/edit';
  import { matchesLogin } from '../lib/data/format';
  import { onReconnect } from '../lib/data/library';
  import { isSafeLinkUrl } from '../lib/safeurl';
  import Dialog from './Dialog.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (text: string) => void; fail: (err: unknown) => void } = $props();

  /** Revealed passwords disappear again after this long. */
  const REVEAL_MS = 30_000;

  // Listed WITHOUT passwords (reveal: false); a password is fetched only after a confirmation.
  let entries = $state<PasswordEntry[]>([]);
  let loaded = $state(false);
  let query = $state('');
  let revealed = $state<Record<number, string>>({});
  const timers = new Map<number, ReturnType<typeof setTimeout>>();
  const shown = $derived(
    entries
      .filter((e) => matchesLogin(e, query))
      .sort((a, b) => a.origin.localeCompare(b.origin) || a.username.localeCompare(b.username)),
  );

  async function refresh() {
    try {
      entries = (await api.call('passwords.list', { reveal: false })).entries;
      loaded = true;
    } catch (err) {
      fail(err);
    }
  }
  $effect(() => {
    void refresh();
    const off = onReconnect(api, () => void refresh());
    return () => {
      off();
      timers.forEach(clearTimeout);
      revealed = {};
    };
  });

  function hide(id: number) {
    clearTimeout(timers.get(id));
    timers.delete(id);
    const rest = { ...revealed };
    delete rest[id];
    revealed = rest;
  }

  // ---- reveal (with confirmation) ---------------------------------------------------------

  let toReveal = $state<PasswordEntry | null>(null);
  let revealOpen = $state(false);
  function askReveal(e: PasswordEntry) {
    toReveal = e;
    revealOpen = true;
  }
  async function reveal() {
    const e = toReveal;
    revealOpen = false;
    if (!e) return;
    try {
      const { entries: found } = await api.call('passwords.list', { origin: e.origin, reveal: true });
      const pw = found.find((x) => x.id === e.id)?.password;
      if (pw === null || pw === undefined) throw new Error(t('library.pw.gone'));
      revealed = { ...revealed, [e.id]: pw };
      clearTimeout(timers.get(e.id));
      timers.set(e.id, setTimeout(() => hide(e.id), REVEAL_MS));
    } catch (err) {
      fail(err);
    }
  }

  async function copy(text: string) {
    try {
      await navigator.clipboard.writeText(text);
      notify(t('library.pw.copied'));
    } catch {
      fail(new Error(t('library.pw.noCopy')));
    }
  }

  // ---- add / edit / delete ----------------------------------------------------------------

  type Form = { origin: string; username: string; password: string; note: string; showPw: boolean };
  let form = $state<Form>({ origin: '', username: '', password: '', note: '', showPw: false });
  let editingEntry = $state<PasswordEntry | null>(null);
  let formOpen = $state(false);
  let formError = $state<string | null>(null);

  function openAdd() {
    editingEntry = null;
    form = { origin: '', username: '', password: '', note: '', showPw: false };
    formError = null;
    formOpen = true;
  }
  function openEdit(e: PasswordEntry) {
    editingEntry = e;
    form = { origin: e.origin, username: e.username, password: '', note: e.note, showPw: false };
    formError = null;
    formOpen = true;
  }
  async function submit() {
    const origin = form.origin.trim();
    if (!origin) return void (formError = t('library.pw.err.site'));
    if (!editingEntry && !form.password) return void (formError = t('library.pw.err.password'));
    try {
      const next = { origin, username: form.username.trim(), password: form.password, note: form.note.trim() };
      if (editingEntry) {
        hide(editingEntry.id);
        await editPassword(api, editingEntry, next);
        notify(t('library.pw.saved'));
      } else {
        const r = await api.call('passwords.save', { ...next, note: next.note || null });
        notify(r.created ? t('library.pw.added') : t('library.pw.updated'));
      }
      formOpen = false;
      await refresh();
    } catch (err) {
      formError = err instanceof Error ? err.message : String(err);
    }
  }

  let toDelete = $state<PasswordEntry | null>(null);
  let deleteOpen = $state(false);
  async function remove() {
    const e = toDelete;
    deleteOpen = false;
    if (!e) return;
    try {
      await api.call('passwords.delete', { id: e.id });
      hide(e.id);
      notify(t('library.pw.deleted', { origin: e.origin }));
      await refresh();
    } catch (err) {
      fail(err);
    }
  }
</script>

<header class="head">
  <h2>{t('library.nav.passwords')}</h2>
  <label class="lib-search">
    <Icon name="search" size={14} />
    <input class="lib-input" type="search" placeholder={t('library.pw.search')} aria-label={t('library.pw.searchLabel')} bind:value={query} />
  </label>
  <div class="spacer"></div>
  <button class="btn primary" onclick={openAdd}><Icon name="plus" size={14} stroke={2.25} /> {t('library.pw.add')}</button>
</header>

<div class="list">
  <p class="note">
    <Icon name="info" size={14} /> {t('library.pw.note')}
  </p>
  {#each shown as e (e.id)}
    {@const pw = revealed[e.id]}
    <div class="lib-row item">
      <Favicon url={e.origin} size={18} />
      <div class="grow site">
        {#if isSafeLinkUrl(e.origin)}
          <a class="title" href={e.origin}>{e.origin.replace(/^https:\/\//, '')}</a>
        {:else}
          <span class="title">{e.origin}</span>
        {/if}
        {#if e.note}<span class="sub">{e.note}</span>{/if}
      </div>
      <span class="user" title={e.username}>{e.username || t('library.pw.noUser')}</span>
      <span class="pw" class:shown={pw !== undefined}>
        {#if pw !== undefined}<code>{pw}</code>{:else}<span aria-label={t('library.pw.hidden')}>••••••••••</span>{/if}
      </span>
      <div class="actions keep">
        {#if pw !== undefined}
          <button class="icon-btn" aria-label={t('library.pw.hide')} title={t('library.pw.hideShort')} onclick={() => hide(e.id)}><Icon name="eye-off" size={15} /></button>
          <button class="icon-btn" aria-label={t('library.pw.copy')} title={t('library.pw.copy')} onclick={() => copy(pw)}><Icon name="copy" size={15} /></button>
        {:else}
          <button class="icon-btn" aria-label={t('library.pw.showFor', { user: e.username, origin: e.origin })} title={t('library.pw.show')} onclick={() => askReveal(e)}>
            <Icon name="eye" size={15} />
          </button>
        {/if}
        <button class="icon-btn" aria-label={t('library.pw.editFor', { origin: e.origin })} title={t('common.edit')} onclick={() => openEdit(e)}><Icon name="pencil" size={15} /></button>
        <button class="icon-btn" aria-label={t('library.pw.deleteFor', { origin: e.origin })} title={t('common.delete')} onclick={() => ((toDelete = e), (deleteOpen = true))}>
          <Icon name="trash" size={15} />
        </button>
      </div>
    </div>
  {:else}
    {#if loaded}
      <div class="lib-empty">
        <h3>{query.trim() ? t('library.pw.noMatch') : t('library.pw.none')}</h3>
        <p>{query.trim() ? t('library.pw.noMatchHelp') : t('library.pw.noneHelp')}</p>
      </div>
    {/if}
  {/each}
</div>

<Dialog bind:open={revealOpen} title={t('library.pw.revealTitle')} onsubmit={reveal}>
  <p>
    {t('library.pw.reveal1')} <strong>{toReveal?.username || t('library.pw.noUser')}</strong> {t('library.pw.reveal2')}
    <strong>{toReveal?.origin}</strong> {t('library.pw.reveal3')}
  </p>
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (revealOpen = false)}>{t('common.cancel')}</button>
    <button type="submit" class="btn primary">{t('library.pw.show')}</button>
  {/snippet}
</Dialog>

<Dialog bind:open={formOpen} title={editingEntry ? t('library.pw.edit') : t('library.pw.add')} onsubmit={submit}>
  <label class="lib-field">
    {t('library.pw.site')}
    <!-- svelte-ignore a11y_autofocus -->
    <input class="lib-input mono" bind:value={form.origin} placeholder="https://example.com" spellcheck="false" autofocus={!editingEntry} />
  </label>
  <label class="lib-field">
    {t('library.pw.username')}
    <input class="lib-input" bind:value={form.username} autocomplete="off" spellcheck="false" />
  </label>
  <label class="lib-field">
    {t('library.pw.password')}
    <span class="pw-field">
      <input
        class="lib-input mono"
        type={form.showPw ? 'text' : 'password'}
        bind:value={form.password}
        autocomplete="new-password"
        placeholder={editingEntry ? t('library.pw.keepCurrent') : ''}
      />
      <button
        type="button"
        class="icon-btn"
        aria-label={form.showPw ? t('library.pw.hideTyped') : t('library.pw.showTyped')}
        onclick={() => (form.showPw = !form.showPw)}
      >
        <Icon name={form.showPw ? 'eye-off' : 'eye'} size={15} />
      </button>
    </span>
  </label>
  <label class="lib-field">
    {t('library.pw.noteField')}
    <input class="lib-input" bind:value={form.note} />
  </label>
  {#if formError}<p class="lib-error">{formError}</p>{/if}
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (formOpen = false)}>{t('common.cancel')}</button>
    <button type="submit" class="btn primary">{t('common.save')}</button>
  {/snippet}
</Dialog>

<Dialog bind:open={deleteOpen} title={t('library.pw.deleteTitle')} onsubmit={remove}>
  <p>
    {t('library.pw.delete1')} <strong>{toDelete?.username || t('library.pw.noUser')}</strong> {t('library.pw.delete2')}
    <strong>{toDelete?.origin}</strong> {t('library.pw.delete3')}
  </p>
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (deleteOpen = false)}>{t('common.cancel')}</button>
    <button type="submit" class="btn primary danger">{t('common.delete')}</button>
  {/snippet}
</Dialog>

<style>
  .head {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 24px 12px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  h2 {
    margin: 0 8px 0 0;
    font-size: 18px;
    font-weight: 650;
  }
  .spacer {
    flex: 1;
  }
  .list {
    max-width: 980px;
    padding: 12px 18px 48px;
  }
  .note {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 10px 10px;
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
  }
  .site {
    flex: 1.3;
  }
  .user {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--fg-muted);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pw {
    width: 190px;
    overflow: hidden;
    color: var(--fg-subtle);
    letter-spacing: 0.1em;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: none;
  }
  .pw.shown {
    color: var(--fg);
    letter-spacing: normal;
  }
  .pw code {
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
  }
  .actions.keep {
    opacity: 1;
  }
  .pw-field {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .pw-field .lib-input {
    flex: 1;
  }
  strong {
    color: var(--fg);
    font-weight: 600;
  }
  @media (max-width: 720px) {
    .head {
      flex-wrap: wrap;
    }
    .pw {
      display: none;
    }
  }
</style>
