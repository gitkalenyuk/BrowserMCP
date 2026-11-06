<script lang="ts">
  import { buildRule, EMPTY_RULE, responseStageAllowed, type RuleForm } from '../../net/logic';
  import { parseRuleLines, type RuleRow } from '../../net/parse';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';

  let { tab }: { tab: number | null } = $props();

  const api = useChrome().api;
  const KINDS = [
    { id: 'block', label: 'Block' },
    { id: 'redirect', label: 'Redirect' },
    { id: 'mock', label: 'Mock response' },
    { id: 'rewrite_body', label: 'Rewrite body' },
  ] as const;
  const METHODS = ['', 'GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS'];

  let rules = $state<RuleRow[]>([]);
  let form = $state<RuleForm>({ ...EMPTY_RULE });
  let error = $state<string | null>(null);
  let adding = $state(false);
  let showForm = $state(false);

  const actionRules = $derived(rules.filter((r) => !r.breakpoint));

  async function refresh() {
    try {
      rules = parseRuleLines((await api.call('net.rule_list', {})).text);
    } catch (err) {
      error = (err as Error).message;
    }
  }

  $effect(() => {
    void refresh();
    const t = setInterval(refresh, 3000); // hit counters
    return () => clearInterval(t);
  });

  async function add() {
    if (tab === null) return;
    const built = buildRule(form);
    if ('error' in built) {
      error = built.error;
      return;
    }
    adding = true;
    error = null;
    try {
      await api.call('net.rule_add', { tab, ...built });
      form = { ...EMPTY_RULE, kind: form.kind };
      showForm = false;
      await refresh();
    } catch (err) {
      error = (err as Error).message;
    } finally {
      adding = false;
    }
  }

  async function remove(id: string) {
    try {
      await api.call('net.rule_remove', { id });
    } catch (err) {
      error = (err as Error).message;
    }
    await refresh();
  }
</script>

<div class="rules">
  <div class="top">
    <p>Rules change matching requests of a tab as they happen: block them, send them elsewhere, answer them from here or edit the body.</p>
    <button class="btn" class:primary={!showForm} disabled={tab === null} onclick={() => (showForm = !showForm)}>
      {#if showForm}Cancel{:else}<Icon name="plus" size={13} stroke={2} /> Add rule{/if}
    </button>
  </div>

  {#if showForm}
    <form
      class="form"
      onsubmit={(e) => {
        e.preventDefault();
        void add();
      }}
    >
      <div class="kinds" role="radiogroup" aria-label="Action">
        {#each KINDS as k (k.id)}
          <button type="button" role="radio" aria-checked={form.kind === k.id} onclick={() => (form.kind = k.id)}>{k.label}</button>
        {/each}
      </div>
      <label class="wide">
        <span>URL {form.regex ? 'regex' : 'pattern'}</span>
        <input bind:value={form.pattern} spellcheck="false" placeholder={form.regex ? '/items/\\d+' : '*://ads.example.com/*  or a part of the URL'} />
      </label>
      <div class="row">
        <label class="check"><input type="checkbox" bind:checked={form.regex} /> Regex</label>
        <label class="inline">
          <span>Method</span>
          <select bind:value={form.method}>
            {#each METHODS as m (m)}<option value={m}>{m || 'Any'}</option>{/each}
          </select>
        </label>
        {#if responseStageAllowed(form.kind)}
          <label class="inline">
            <span>Stage</span>
            <select bind:value={form.stage}>
              <option value="request">Request</option>
              <option value="response">Response</option>
            </select>
          </label>
        {/if}
      </div>
      {#if form.kind === 'redirect'}
        <label class="wide"><span>Redirect to</span><input bind:value={form.redirectUrl} type="url" spellcheck="false" placeholder="https://…" /></label>
      {:else if form.kind === 'mock'}
        <div class="row">
          <label class="inline"><span>Status</span><input class="num" bind:value={form.mockStatus} inputmode="numeric" /></label>
          <label class="inline grow"><span>Content-Type</span><input bind:value={form.mockContentType} spellcheck="false" placeholder="guessed from the body" /></label>
        </div>
        <label class="wide"><span>Body</span><textarea bind:value={form.mockBody} rows="4" spellcheck="false" placeholder={'{"ok": true}'}></textarea></label>
        <p class="hint">Mocks answer at the request stage: the request is never sent.</p>
      {:else if form.kind === 'rewrite_body'}
        <label class="wide"><span>Find</span><input bind:value={form.find} spellcheck="false" /></label>
        <label class="wide"><span>Replace with</span><input bind:value={form.replace} spellcheck="false" /></label>
      {/if}
      <div class="actions">
        <button class="btn primary" type="submit" disabled={adding}>{adding ? 'Adding…' : `Add to tab ${tab ?? ''}`}</button>
      </div>
    </form>
  {/if}

  {#if error}
    <p class="err" role="alert">{error}</p>
  {/if}

  <ul class="list">
    {#each actionRules as r (r.id)}
      <li>
        <span class="id">{r.id}</span>
        <div class="what">
          <span class="action">{r.action}</span>
          <span class="match">{r.stage} · {r.match} · tab {r.tab}</span>
        </div>
        <span class="hits" title="Requests matched">{r.hits}</span>
        <button class="icon-btn" aria-label="Remove rule {r.id}" title="Remove" onclick={() => remove(r.id)}><Icon name="trash" size={13} /></button>
      </li>
    {:else}
      <li class="none">No rules. Breakpoints are on their own tab.</li>
    {/each}
  </ul>
</div>

<style>
  .rules {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
  .top {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px;
  }
  .top p {
    flex: 1;
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    line-height: 1.45;
  }
  .form {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 0 10px 10px;
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--surface);
  }
  .kinds {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .kinds button {
    height: 24px;
    padding: 0 9px;
    border: 1px solid var(--border);
    border-radius: 12px;
    background: transparent;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .kinds button[aria-checked='true'] {
    border-color: var(--accent-line);
    background: var(--accent-soft);
    color: var(--fg);
  }
  label {
    display: flex;
    gap: 4px;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .wide {
    flex-direction: column;
  }
  .inline {
    align-items: center;
    gap: 6px;
  }
  .grow {
    flex: 1;
  }
  .grow input {
    flex: 1;
  }
  .check {
    align-items: center;
    gap: 5px;
    font-weight: 500;
  }
  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
  }
  input:not([type='checkbox']),
  select,
  textarea {
    min-width: 0;
    height: 26px;
    padding: 0 8px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--field-focus);
    color: var(--fg);
    font: var(--fs-xs) var(--font-mono);
    user-select: text;
  }
  select {
    font-family: var(--font);
  }
  textarea {
    height: auto;
    padding: 6px 8px;
    resize: vertical;
  }
  .num {
    width: 56px;
  }
  .hint {
    margin: 0;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
  }
  .err {
    margin: 0 10px 8px;
    color: var(--danger);
    font-size: var(--fs-xs);
  }
  .list {
    margin: 0;
    padding: 0;
    list-style: none;
    border-top: 1px solid var(--border);
  }
  .list li {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 6px 5px 10px;
    border-bottom: 1px solid var(--border);
  }
  .list .none {
    padding: 14px 10px;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
  .id {
    color: var(--fg-subtle);
    font: var(--fs-xs) var(--font-mono);
  }
  .what {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
  }
  .action {
    font: 600 var(--fs-xs) var(--font-mono);
    overflow-wrap: anywhere;
  }
  .match {
    color: var(--fg-subtle);
    font: var(--fs-xs) var(--font-mono);
    overflow-wrap: anywhere;
  }
  .hits {
    min-width: 20px;
    padding: 0 6px;
    border-radius: 9px;
    background: var(--sunken);
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    text-align: center;
  }
  .list .icon-btn {
    width: 26px;
    height: 26px;
  }
</style>
