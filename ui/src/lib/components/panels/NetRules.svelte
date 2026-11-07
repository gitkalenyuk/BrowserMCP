<script lang="ts">
  import { t } from '../../i18n/index.svelte';
  import { buildRule, EMPTY_RULE, responseStageAllowed, type RuleForm } from '../../net/logic';
  import { parseRuleLines, type RuleRow } from '../../net/parse';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';

  let { tab }: { tab: number | null } = $props();

  const api = useChrome().api;
  // Labels: `net.kind.<id>`.
  const KINDS = ['block', 'redirect', 'mock', 'rewrite_body'] as const;
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
    const timer = setInterval(refresh, 3000); // hit counters
    return () => clearInterval(timer);
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
    <p>{t('net.rules.intro')}</p>
    <button class="btn" class:primary={!showForm} disabled={tab === null} onclick={() => (showForm = !showForm)}>
      {#if showForm}{t('common.cancel')}{:else}<Icon name="plus" size={13} stroke={2} /> {t('net.rules.add')}{/if}
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
      <div class="kinds" role="radiogroup" aria-label={t('net.rules.action')}>
        {#each KINDS as k (k)}
          <button type="button" role="radio" aria-checked={form.kind === k} onclick={() => (form.kind = k)}>{t(`net.kind.${k}`)}</button>
        {/each}
      </div>
      <label class="wide">
        <span>{form.regex ? t('net.rules.urlRegex') : t('net.rules.urlPattern')}</span>
        <input bind:value={form.pattern} spellcheck="false" placeholder={form.regex ? '/items/\\d+' : t('net.rules.patternPlaceholder')} />
      </label>
      <div class="row">
        <label class="check"><input type="checkbox" bind:checked={form.regex} /> {t('net.rules.regex')}</label>
        <label class="inline">
          <span>{t('net.col.method')}</span>
          <select bind:value={form.method}>
            {#each METHODS as m (m)}<option value={m}>{m || t('net.rules.anyMethod')}</option>{/each}
          </select>
        </label>
        {#if responseStageAllowed(form.kind)}
          <label class="inline">
            <span>{t('net.rules.stage')}</span>
            <select bind:value={form.stage}>
              <option value="request">{t('net.stage.request')}</option>
              <option value="response">{t('net.stage.response')}</option>
            </select>
          </label>
        {/if}
      </div>
      {#if form.kind === 'redirect'}
        <label class="wide"><span>{t('net.rules.redirectTo')}</span><input bind:value={form.redirectUrl} type="url" spellcheck="false" placeholder="https://…" /></label>
      {:else if form.kind === 'mock'}
        <div class="row">
          <label class="inline"><span>{t('net.col.status')}</span><input class="num" bind:value={form.mockStatus} inputmode="numeric" /></label>
          <label class="inline grow"><span>Content-Type</span><input bind:value={form.mockContentType} spellcheck="false" placeholder={t('net.rules.guessed')} /></label>
        </div>
        <label class="wide"><span>{t('net.rules.body')}</span><textarea bind:value={form.mockBody} rows="4" spellcheck="false" placeholder={'{"ok": true}'}></textarea></label>
        <p class="hint">{t('net.rules.mockHint')}</p>
      {:else if form.kind === 'rewrite_body'}
        <label class="wide"><span>{t('net.rules.find')}</span><input bind:value={form.find} spellcheck="false" /></label>
        <label class="wide"><span>{t('net.rules.replace')}</span><input bind:value={form.replace} spellcheck="false" /></label>
      {/if}
      <div class="actions">
        <button class="btn primary" type="submit" disabled={adding}>{adding ? t('net.rules.adding') : t('net.rules.addToTab', { tab: tab ?? '' })}</button>
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
          <span class="match">{t(`net.stage.${r.stage}`)} · {r.match} · {t('record.tabN', { id: r.tab })}</span>
        </div>
        <span class="hits" title={t('net.rules.hits')}>{r.hits}</span>
        <button class="icon-btn" aria-label={t('net.rules.remove', { id: r.id })} title={t('runs.trg.removeShort')} onclick={() => remove(r.id)}><Icon name="trash" size={13} /></button>
      </li>
    {:else}
      <li class="none">{t('net.rules.none')}</li>
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
