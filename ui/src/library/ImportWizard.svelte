<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import { t, tp } from '../lib/i18n/index.svelte';
  import type { ImportPartResult } from '../lib/api/types-data';
  import Icon from '../lib/components/Icon.svelte';
  import { markImportOffered } from '../lib/data/library';
  import { richParts } from '../lib/settings/rich';
  import {
    canStart,
    cleanPath,
    importArgs,
    initialWizard,
    problems,
    selectedProfile,
    wizardReducer,
    type WizardEvent,
    type WizardPart,
  } from '../lib/data/wizard';

  let { api, onfinish }: { api: BrowserApi; onfinish: () => void } = $props();

  let s = $state(initialWizard());
  const send = (e: WizardEvent) => (s = wizardReducer(s, e));
  const issues = $derived(problems(s));
  const profile = $derived(selectedProfile(s));

  $effect(() => {
    // Seen once: the first-run prompt does not come back, whatever the user picks.
    markImportOffered();
    api
      .call('import.chrome_detect', {})
      .then((r) => send({ type: 'detected', profiles: r.profiles, userDataDirs: r.user_data_dirs }))
      .catch((err) => send({ type: 'detect_failed', error: err instanceof Error ? err.message : String(err) }));
  });

  async function start() {
    const args = importArgs(s);
    if (!args || !canStart(s)) return;
    send({ type: 'start' });
    try {
      const { results } = await api.call('import.chrome', args);
      send({ type: 'finished', results });
    } catch (err) {
      send({ type: 'failed', error: err instanceof Error ? err.message : String(err) });
    }
  }

  const toggle = (part: WizardPart) => (e: Event) => send({ type: 'toggle', part, on: (e.currentTarget as HTMLInputElement).checked });
  const anyData = $derived(s.profiles.some((p) => p.has_bookmarks || p.has_history));

  // The texts of lib/data/wizard (problems, PART_LABELS, resultSummary) are English; the
  // same checks and summaries in the interface language:
  /** Why the import cannot start yet (null = ready). A check added to `problems` later shows as is. */
  function issueText(): string | null {
    if (!(s.parts.browsing || s.parts.passwords || s.parts.cookies)) return t('library.imp.err.nothing');
    if (s.parts.browsing) {
      const p = selectedProfile(s);
      if (!p) return t('library.imp.err.profile');
      if (!(p.has_bookmarks || p.has_history)) return t('library.imp.err.noData', { name: p.name });
    }
    if (s.parts.passwords && !cleanPath(s.passwordsCsv)) return t('library.imp.err.csv');
    if (s.parts.cookies && !cleanPath(s.cookiesJson)) return t('library.imp.err.cookies');
    return issues[0] ?? null;
  }
  /** "12 imported, 3 skipped" for one result row. */
  function summary(r: ImportPartResult): string {
    if (!r.ok) return r.error ?? t('library.imp.failed');
    return tp('library.imp.imported', r.imported) + (r.skipped ? `, ${tp('library.imp.skipped', r.skipped)}` : '');
  }
  const busy = $derived(s.step === 'running');
</script>

<div class="wiz">
  <header>
    <span class="badge"><Icon name="import" size={20} /></span>
    <div>
      <h2>{t('library.imp.title')}</h2>
      <p>{t('library.imp.intro')}</p>
    </div>
  </header>

  {#if s.step === 'detecting'}
    <p class="muted" role="status">{t('library.imp.detecting')}</p>
  {:else if s.step === 'skipped'}
    <div class="card done">
      <h3>{t('library.imp.nothing')}</h3>
      <p class="muted">{t('library.imp.later')}</p>
      <div class="row">
        <button class="btn" onclick={() => send({ type: 'back' })}>{t('library.imp.again')}</button>
        <button class="btn primary" onclick={onfinish}>{t('library.imp.toBookmarks')}</button>
      </div>
    </div>
  {:else if s.step === 'done'}
    <div class="card done">
      <h3>{t('library.imp.finished')}</h3>
      <ul class="results">
        {#each s.results as r (r.part)}
          <li>
            <span class="lib-chip" class:ok={r.ok} class:bad={!r.ok}>
              <Icon name={r.ok ? 'check' : 'alert'} size={12} stroke={2.5} />
            </span>
            <strong>{t(`library.imp.part.${r.part}`)}</strong>
            <span class:err={!r.ok}>{summary(r)}</span>
            {#if r.source}<span class="src" title={r.source}>{r.source}</span>{/if}
          </li>
        {/each}
      </ul>
      <div class="row">
        <button class="btn" onclick={() => send({ type: 'back' })}>{t('library.imp.more')}</button>
        <button class="btn primary" onclick={onfinish}>{t('library.imp.toBookmarks')}</button>
      </div>
    </div>
  {:else}
    <fieldset class="card" disabled={busy}>
      <legend class="sr-only">{t('library.imp.what')}</legend>

      <label class="part">
        <input type="checkbox" checked={s.parts.browsing} disabled={!anyData} onchange={toggle('browsing')} />
        <span class="text">
          <strong>{t('library.imp.browsing')}</strong>
          <span class="muted">{t('library.imp.browsingHelp')}</span>
          {#if s.detectError}
            <span class="lib-error">{t('library.imp.detectError', { error: s.detectError })}</span>
          {:else if !s.profiles.length}
            <span class="muted">{s.userDataDirs.length ? t('library.imp.noProfileIn', { dirs: s.userDataDirs.join(', ') }) : t('library.imp.noProfile')}</span>
          {/if}
        </span>
      </label>
      {#if s.profiles.length}
        <div class="profiles" role="radiogroup" aria-label={t('library.imp.profile')}>
          {#each s.profiles as p (p.path)}
            <label class="profile" class:off={!(p.has_bookmarks || p.has_history)}>
              <input
                type="radio"
                name="profile"
                checked={s.profile === p.path}
                disabled={!s.parts.browsing}
                onchange={() => send({ type: 'select_profile', path: p.path })}
              />
              <span class="text">
                <strong>{p.name}</strong>
                <span class="muted mono" title={p.path}>{p.path}</span>
              </span>
              <span class="tags">
                {#if p.has_bookmarks}<span class="lib-chip">{t('library.imp.tag.bookmarks')}</span>{/if}
                {#if p.has_history}<span class="lib-chip">{t('library.imp.tag.history')}</span>{/if}
                {#if !(p.has_bookmarks || p.has_history)}<span class="lib-chip">{t('library.imp.tag.empty')}</span>{/if}
              </span>
            </label>
          {/each}
        </div>
      {/if}

      <label class="part">
        <input type="checkbox" checked={s.parts.passwords} onchange={toggle('passwords')} />
        <span class="text">
          <strong>{t('library.imp.part.passwords_csv')}</strong>
          <span class="muted">{t('library.imp.passwordsHelp')}</span>
        </span>
      </label>
      {#if s.parts.passwords}
        <div class="sub">
          <ol class="steps">
            <li>{#each richParts(t('library.imp.step1')) as p, i (i)}{#if p.slot === 'url'}<code>chrome://password-manager/settings</code>{:else}{p.text}{/if}{/each}</li>
            <li>{t('library.imp.step2a')} <em>{t('library.imp.chromeExport')}</em> {t('library.imp.step2b')} <em>{t('library.imp.chromeDownload')}</em> {t('library.imp.step2c')}</li>
            <li>{t('library.imp.step3a')} <code>.csv</code> {t('library.imp.step3b')} <em>{t('library.imp.copyAsPath')}</em>{t('library.imp.step3c')}</li>
          </ol>
          <label class="lib-field">
            {t('library.imp.csv')}
            <input
              class="lib-input mono"
              value={s.passwordsCsv}
              oninput={(e) => send({ type: 'set_path', part: 'passwords', path: e.currentTarget.value })}
              placeholder={t('library.imp.csvPlaceholder')}
              spellcheck="false"
            />
          </label>
          <p class="muted small">{t('library.imp.deleteCsv')}</p>
        </div>
      {/if}

      <label class="part">
        <input type="checkbox" checked={s.parts.cookies} onchange={toggle('cookies')} />
        <span class="text">
          <strong>{t('library.imp.cookies')}</strong>
          <span class="muted">{t('library.imp.cookiesHelp')}</span>
        </span>
      </label>
      {#if s.parts.cookies}
        <div class="sub">
          <label class="lib-field">
            {t('library.imp.json')}
            <input
              class="lib-input mono"
              value={s.cookiesJson}
              oninput={(e) => send({ type: 'set_path', part: 'cookies', path: e.currentTarget.value })}
              placeholder={t('library.imp.cookiesPlaceholder')}
              spellcheck="false"
            />
          </label>
        </div>
      {/if}

      {#if s.error}<p class="lib-error" role="alert">{s.error}</p>{/if}

      <div class="row foot">
        <span class="muted small">
          {#if busy}
            {t('library.imp.running')}
          {:else if issues.length}
            {issueText()}
          {:else}
            {profile && s.parts.browsing ? t('library.imp.readyFrom', { name: profile.name }) : t('library.imp.ready')}
          {/if}
        </span>
        <button type="button" class="btn" onclick={() => send({ type: 'skip' })}>{t('library.imp.skip')}</button>
        <button type="button" class="btn primary" disabled={!canStart(s)} onclick={start}>
          {busy ? t('library.imp.running') : t('library.imp.start')}
        </button>
      </div>
    </fieldset>
  {/if}
</div>

<style>
  .wiz {
    max-width: 760px;
    padding: 28px 28px 56px;
  }
  header {
    display: flex;
    gap: 14px;
    margin-bottom: 18px;
  }
  .badge {
    display: inline-grid;
    place-items: center;
    width: 42px;
    height: 42px;
    border-radius: 12px;
    background: var(--accent-soft);
    color: var(--accent);
    flex: none;
  }
  h2 {
    margin: 2px 0 4px;
    font-size: 20px;
    font-weight: 650;
  }
  header p {
    margin: 0;
    color: var(--fg-muted);
  }
  .card {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0;
    padding: 14px;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--toolbar);
    min-inline-size: 0;
  }
  .card.done {
    gap: 10px;
  }
  h3 {
    margin: 0;
    font-size: var(--fs-lg);
  }
  .part {
    display: flex;
    gap: 10px;
    padding: 10px 6px;
    border-radius: var(--r);
  }
  .part + .part,
  .sub + .part,
  .profiles + .part {
    border-top: 1px solid var(--border);
  }
  .part input {
    margin-top: 2px;
    accent-color: var(--accent);
  }
  .text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .muted {
    color: var(--fg-subtle);
  }
  .small {
    font-size: var(--fs-sm);
  }
  .mono {
    overflow: hidden;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .profiles {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin: -4px 0 8px 30px;
  }
  .profile {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 10px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
  }
  .profile.off {
    opacity: 0.6;
  }
  .profile .text {
    flex: 1;
  }
  .profile input {
    accent-color: var(--accent);
  }
  .tags {
    display: flex;
    gap: 4px;
  }
  .sub {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: -4px 0 8px 30px;
  }
  .steps {
    margin: 0;
    padding-left: 18px;
    color: var(--fg-muted);
  }
  .steps li + li {
    margin-top: 3px;
  }
  code {
    padding: 0 4px;
    border-radius: 4px;
    background: var(--sunken);
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    user-select: all;
  }
  .row {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
  }
  .foot {
    padding-top: 10px;
    border-top: 1px solid var(--border);
  }
  .foot .muted {
    flex: 1;
  }
  .results {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .results li {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .results .err {
    color: var(--danger);
  }
  .src {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--fg-subtle);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }
</style>
