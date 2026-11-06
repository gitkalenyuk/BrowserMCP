<script lang="ts">
  import type { AiProvider, AiProviderSet, AiProviderTest } from '../../api';
  import type { AiStore } from '../../ai/store.svelte';
  import { providerHint } from '../../providers';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';

  /**
   * `store`: the settings page has no chrome controller and passes its own AiStore; the side
   * panel uses the chrome's. Without `onback` there is no back bar (full page).
   */
  let { onback, store }: { onback?: () => void; store?: AiStore } = $props();

  // svelte-ignore state_referenced_locally
  const ai = store ?? useChrome().ai;
  // svelte-ignore state_referenced_locally
  let openId = $state<string | null>(ai.provider);
  let form = $state({ apiKey: '', baseUrl: '', model: '', path: '' });
  let busy = $state<'save' | 'test' | null>(null);
  let result = $state<{ ok: boolean; text: string } | null>(null);

  const selected = $derived(ai.providers.find((p) => p.id === openId) ?? null);

  function edit(p: AiProvider) {
    if (openId === p.id) {
      openId = null;
      return;
    }
    openId = p.id;
    reset(p);
  }

  function reset(p: AiProvider) {
    form = { apiKey: '', baseUrl: p.base_url, model: p.model ?? '', path: p.path ?? '' };
    result = null;
  }

  $effect(() => {
    // Fill the form once the provider list arrives.
    if (selected && form.baseUrl === '' && form.model === '' && form.path === '' && !result) reset(selected);
  });

  /** Only the fields the user changed ("" clears one in the core). */
  function changes(p: AiProvider): AiProviderSet {
    const args: AiProviderSet = { id: p.id };
    if (form.apiKey.trim()) args.api_key = form.apiKey.trim();
    if (form.baseUrl.trim() !== p.base_url) args.base_url = form.baseUrl.trim();
    if (form.model.trim() !== (p.model ?? '')) args.model = form.model.trim();
    if (p.kind === 'cli' && form.path.trim() !== (p.path ?? '')) args.path = form.path.trim();
    return args;
  }

  async function save(p: AiProvider) {
    busy = 'save';
    result = null;
    try {
      const next = await ai.saveProvider(changes(p));
      reset(next);
      result = { ok: true, text: next.configured ? 'Saved. Ready to use.' : 'Saved, but not ready yet.' };
    } catch (err) {
      result = { ok: false, text: (err as Error).message };
    } finally {
      busy = null;
    }
  }

  async function removeKey(p: AiProvider) {
    busy = 'save';
    try {
      reset(await ai.saveProvider({ id: p.id, api_key: '' }));
      result = { ok: true, text: 'API key removed.' };
    } catch (err) {
      result = { ok: false, text: (err as Error).message };
    } finally {
      busy = null;
    }
  }

  async function makeDefault(p: AiProvider) {
    busy = 'save';
    try {
      await ai.saveProvider({ id: p.id, default: true });
      await ai.refreshProviders(); // the previous default lost its flag
      result = { ok: true, text: `${p.name} now answers self-healing and page.extract.` };
    } catch (err) {
      result = { ok: false, text: (err as Error).message };
    } finally {
      busy = null;
    }
  }

  function describe(t: AiProviderTest): string {
    if (!t.ok) return t.error ?? 'Failed';
    if (t.version) return `OK · ${t.version}`;
    return t.models.length ? `OK · ${t.models.length} models: ${t.models.slice(0, 6).join(', ')}${t.models.length > 6 ? '…' : ''}` : 'OK';
  }

  async function test(p: AiProvider) {
    busy = 'test';
    result = null;
    try {
      const t = await ai.testProvider(p.id);
      result = { ok: t.ok, text: describe(t) };
    } catch (err) {
      result = { ok: false, text: (err as Error).message };
    } finally {
      busy = null;
    }
  }
</script>

<div class="settings" class:page={!onback}>
  {#if onback}
    <div class="bar">
      <button class="icon-btn" aria-label="Back to chat" title="Back" onclick={onback}><Icon name="arrow-left" size={15} /></button>
      <h3>Providers</h3>
    </div>
  {/if}
  <div class="list">
    {#if ai.providersError}
      <p class="err">Could not load providers: {ai.providersError}</p>
    {/if}
    {#each ai.providers as p (p.id)}
      <section class="prov" class:open={openId === p.id}>
        <button class="row" aria-expanded={openId === p.id} onclick={() => edit(p)}>
          <span class="name">{p.name}</span>
          <span class="hint">{p.kind === 'cli' ? 'CLI agent' : providerHint(p.id)}</span>
          {#if p.is_default}<span class="badge def" title="Used by self-healing and page.extract when they name no provider">Default</span>{/if}
          <span class="badge" class:ready={p.configured}>{p.configured ? 'Ready' : p.kind === 'cli' ? 'Not found' : 'Not set up'}</span>
          {#if ai.provider === p.id}<span class="sel" title="Used for new chats"><Icon name="check" size={13} stroke={2.25} /></span>{/if}
        </button>
        {#if openId === p.id}
          <form
            class="form"
            onsubmit={(e) => {
              e.preventDefault();
              void save(p);
            }}
          >
            {#if p.kind === 'api'}
              <label>
                <span>API key{p.needs_key ? '' : ' (optional)'}</span>
                <span class="key">
                  <input
                    type="password"
                    autocomplete="off"
                    spellcheck="false"
                    bind:value={form.apiKey}
                    placeholder={p.has_key ? '•••••••• stored — type to replace' : p.needs_key ? 'Paste your key' : 'Not needed'}
                  />
                  {#if p.has_key}
                    <button type="button" class="btn" disabled={busy !== null} onclick={() => removeKey(p)}>Remove</button>
                  {/if}
                </span>
              </label>
              <label>
                <span>Base URL</span>
                <input type="url" spellcheck="false" bind:value={form.baseUrl} placeholder="https://…/v1" />
              </label>
            {:else}
              <label>
                <span>Program</span>
                <input spellcheck="false" bind:value={form.path} placeholder="Search PATH for {p.id === 'codex' ? 'codex' : 'claude'}" />
              </label>
            {/if}
            <label>
              <span>Default model</span>
              <input spellcheck="false" bind:value={form.model} list="models-{p.id}" placeholder={p.models[0] ?? 'model id'} />
              <datalist id="models-{p.id}">
                {#each p.models as m (m)}<option value={m}></option>{/each}
              </datalist>
            </label>
            <p class="note">Keys are saved in data/config/ai.json on this computer. Test lists the models (or runs --version) and spends no tokens.</p>
            <div class="actions">
              <button class="btn primary" type="submit" disabled={busy !== null}>{busy === 'save' ? 'Saving…' : 'Save'}</button>
              <button class="btn" type="button" disabled={busy !== null} onclick={() => test(p)}>{busy === 'test' ? 'Testing…' : 'Test'}</button>
              <button class="btn" type="button" disabled={ai.provider === p.id} onclick={() => ai.setProvider(p.id)}>
                {ai.provider === p.id ? 'In use' : 'Use for new chats'}
              </button>
              <button
                class="btn"
                type="button"
                disabled={busy !== null || !!p.is_default}
                title="Background AI features (self-healing of runs, page.extract) use this provider when they name none"
                onclick={() => makeDefault(p)}
              >
                {p.is_default ? 'Default for background AI' : 'Make default for background AI'}
              </button>
            </div>
            {#if result && result.text}
              <p class="result" class:bad={!result.ok} role="status">{result.text}</p>
            {/if}
          </form>
        {/if}
      </section>
    {:else}
      {#if !ai.providersError}<p class="muted">Loading providers…</p>{/if}
    {/each}
  </div>
</div>

<style>
  .settings {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 6px;
    min-height: 44px;
    padding: 6px 10px 6px 6px;
    border-bottom: 1px solid var(--border);
  }
  h3 {
    margin: 0;
    font-size: var(--fs);
    font-weight: 650;
  }
  .list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 6px 0;
  }
  .prov.open {
    background: var(--surface);
    border-block: 1px solid var(--border);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 34px;
    padding: 0 12px;
    border: 0;
    background: transparent;
    text-align: left;
  }
  .row:hover {
    background: var(--hover);
  }
  .name {
    font-weight: 600;
    white-space: nowrap;
  }
  .hint {
    flex: 1;
    min-width: 0;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .badge {
    padding: 1px 7px;
    border-radius: 9px;
    background: var(--sunken);
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    font-weight: 600;
    white-space: nowrap;
  }
  .badge.def {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .settings.page .list {
    overflow: visible;
    padding: 0;
  }
  .badge.ready {
    background: color-mix(in oklab, var(--ok) 14%, transparent);
    color: var(--ok);
  }
  .sel {
    display: grid;
    color: var(--accent);
  }
  .form {
    display: flex;
    flex-direction: column;
    gap: 9px;
    padding: 4px 12px 12px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: var(--fs-xs);
    font-weight: 600;
    color: var(--fg-muted);
  }
  .key {
    display: flex;
    gap: 6px;
  }
  input {
    flex: 1;
    min-width: 0;
    height: 28px;
    padding: 0 9px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--field-focus);
    color: var(--fg);
    font-size: var(--fs-sm);
    font-weight: 400;
    user-select: text;
  }
  input:focus {
    outline: none;
    border-color: var(--accent-line);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .note,
  .muted {
    margin: 0;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    line-height: 1.45;
  }
  .muted {
    padding: 12px;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .result {
    margin: 0;
    color: var(--ok);
    font-size: var(--fs-xs);
    overflow-wrap: anywhere;
  }
  .result.bad,
  .err {
    color: var(--danger);
  }
  .err {
    margin: 8px 12px;
    font-size: var(--fs-xs);
  }
</style>
