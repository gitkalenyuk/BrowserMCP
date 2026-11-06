<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import Icon from '../lib/components/Icon.svelte';
  import { bumpDataRev, markImportOffered } from '../lib/data/library';
  import {
    PART_LABELS,
    canStart,
    importArgs,
    initialWizard,
    problems,
    resultSummary,
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
    } finally {
      bumpDataRev();
    }
  }

  const toggle = (part: WizardPart) => (e: Event) => send({ type: 'toggle', part, on: (e.currentTarget as HTMLInputElement).checked });
  const anyData = $derived(s.profiles.some((p) => p.has_bookmarks || p.has_history));
  const busy = $derived(s.step === 'running');
</script>

<div class="wiz">
  <header>
    <span class="badge"><Icon name="import" size={20} /></span>
    <div>
      <h2>Bring your stuff from Chrome</h2>
      <p>Pick what to copy over. Nothing in Chrome is changed, and you can run this again any time from here.</p>
    </div>
  </header>

  {#if s.step === 'detecting'}
    <p class="muted" role="status">Looking for Chrome profiles…</p>
  {:else if s.step === 'skipped'}
    <div class="card done">
      <h3>Nothing imported</h3>
      <p class="muted">You can import later from Library → Import from Chrome.</p>
      <div class="row">
        <button class="btn" onclick={() => send({ type: 'back' })}>Choose again</button>
        <button class="btn primary" onclick={onfinish}>Go to bookmarks</button>
      </div>
    </div>
  {:else if s.step === 'done'}
    <div class="card done">
      <h3>Import finished</h3>
      <ul class="results">
        {#each s.results as r (r.part)}
          <li>
            <span class="lib-chip" class:ok={r.ok} class:bad={!r.ok}>
              <Icon name={r.ok ? 'check' : 'alert'} size={12} stroke={2.5} />
            </span>
            <strong>{PART_LABELS[r.part]}</strong>
            <span class:err={!r.ok}>{resultSummary(r)}</span>
            {#if r.source}<span class="src" title={r.source}>{r.source}</span>{/if}
          </li>
        {/each}
      </ul>
      <div class="row">
        <button class="btn" onclick={() => send({ type: 'back' })}>Import something else</button>
        <button class="btn primary" onclick={onfinish}>Go to bookmarks</button>
      </div>
    </div>
  {:else}
    <fieldset class="card" disabled={busy}>
      <legend class="sr-only">What to import</legend>

      <label class="part">
        <input type="checkbox" checked={s.parts.browsing} disabled={!anyData} onchange={toggle('browsing')} />
        <span class="text">
          <strong>Bookmarks and history</strong>
          <span class="muted">Read straight from a Chrome profile folder. Close Chrome first if the history is locked.</span>
          {#if s.detectError}
            <span class="lib-error">Could not look for Chrome: {s.detectError}</span>
          {:else if !s.profiles.length}
            <span class="muted">No Chrome profile found{s.userDataDirs.length ? ` in ${s.userDataDirs.join(', ')}` : ''}.</span>
          {/if}
        </span>
      </label>
      {#if s.profiles.length}
        <div class="profiles" role="radiogroup" aria-label="Chrome profile">
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
                {#if p.has_bookmarks}<span class="lib-chip">bookmarks</span>{/if}
                {#if p.has_history}<span class="lib-chip">history</span>{/if}
                {#if !(p.has_bookmarks || p.has_history)}<span class="lib-chip">empty</span>{/if}
              </span>
            </label>
          {/each}
        </div>
      {/if}

      <label class="part">
        <input type="checkbox" checked={s.parts.passwords} onchange={toggle('passwords')} />
        <span class="text">
          <strong>Passwords</strong>
          <span class="muted">Chrome encrypts its passwords, so they come from a file you export:</span>
        </span>
      </label>
      {#if s.parts.passwords}
        <div class="sub">
          <ol class="steps">
            <li>In Chrome, open <code>chrome://password-manager/settings</code> (type it in Chrome's address bar).</li>
            <li>Under <em>Export passwords</em> click <em>Download file</em> and confirm with your Windows password.</li>
            <li>Paste the path of the saved <code>.csv</code> here (in Explorer: Shift + right-click the file → <em>Copy as path</em>).</li>
          </ol>
          <label class="lib-field">
            Passwords CSV
            <input
              class="lib-input mono"
              value={s.passwordsCsv}
              oninput={(e) => send({ type: 'set_path', part: 'passwords', path: e.currentTarget.value })}
              placeholder="C:\Users\you\Downloads\Chrome Passwords.csv"
              spellcheck="false"
            />
          </label>
          <p class="muted small">Delete the CSV afterwards: it holds every password in plain text.</p>
        </div>
      {/if}

      <label class="part">
        <input type="checkbox" checked={s.parts.cookies} onchange={toggle('cookies')} />
        <span class="text">
          <strong>Cookies (stay signed in)</strong>
          <span class="muted">A JSON file from a cookie-export extension (an array of name, value, domain, path…).</span>
        </span>
      </label>
      {#if s.parts.cookies}
        <div class="sub">
          <label class="lib-field">
            Cookies JSON
            <input
              class="lib-input mono"
              value={s.cookiesJson}
              oninput={(e) => send({ type: 'set_path', part: 'cookies', path: e.currentTarget.value })}
              placeholder="C:\Users\you\Downloads\cookies.json"
              spellcheck="false"
            />
          </label>
        </div>
      {/if}

      {#if s.error}<p class="lib-error" role="alert">{s.error}</p>{/if}

      <div class="row foot">
        <span class="muted small">
          {#if busy}
            Importing…
          {:else if issues.length}
            {issues[0]}
          {:else}
            Ready{profile && s.parts.browsing ? ` — from “${profile.name}”` : ''}.
          {/if}
        </span>
        <button type="button" class="btn" onclick={() => send({ type: 'skip' })}>Import nothing</button>
        <button type="button" class="btn primary" disabled={!canStart(s)} onclick={start}>
          {busy ? 'Importing…' : 'Import'}
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
