<script lang="ts">
  import { untrack } from 'svelte';
  import type { AiProvider, AiProviderTest } from '../../api';
  import type { AiProblem } from '../../api/types-ai';
  import type { AiStore } from '../../ai/store.svelte';
  import { draftFor, dropDraft, keepDraft, NEW_DRAFT } from '../../ai/provider-draft';
  import { addArgs, changes, formFrom, formFromTemplate, isDirty, isDraftDirty, isUser, MAX_NAME, rebase, validate, type ProviderForm } from '../../ai/provider-form';
  import { t, tp } from '../../i18n/index.svelte';
  import { APIS, BLANK_TEMPLATE } from '../../providers';
  import Icon from '../Icon.svelte';

  /**
   * The settings form of one provider (built-in, the user's own, or — with `provider` null —
   * a new one made from `template`). `dirty` tells the list whether closing it loses edits.
   * Mounted fresh for each provider; when the provider changes underneath (a test loaded its
   * models, a save elsewhere), untouched fields follow it and edits are kept.
   */
  let {
    ai,
    provider = null,
    template = null,
    notice = null,
    dirty = $bindable(false),
    oncreated,
    onremoved,
    oncancel,
  }: {
    ai: AiStore;
    provider?: AiProvider | null;
    /** New provider: the built-in API provider to start from; null = blank. */
    template?: AiProvider | null;
    /** Shown as the first result (e.g. "Added" after the list reopens a new provider). */
    notice?: string | null;
    dirty?: boolean;
    oncreated?: (p: AiProvider) => void;
    onremoved?: (id: string) => void;
    oncancel?: () => void;
  } = $props();

  const OTHER_MODEL = '\u0000other';
  const EXAMPLE_URL = 'http://localhost:11434/v1';
  /** i18n keys of what ai.provider_test reports missing (it sent nothing then). */
  const PROBLEM: Record<AiProblem, string> = {
    no_base_url: 'aiProviders.problem.noBaseUrl',
    no_api_key: 'aiProviders.problem.noApiKey',
    not_installed: 'aiProviders.problem.notInstalled',
  };

  // svelte-ignore state_referenced_locally
  const isNew = provider === null;
  // svelte-ignore state_referenced_locally
  const draftKey = provider?.id ?? NEW_DRAFT;
  // svelte-ignore state_referenced_locally
  const templateKey = isNew ? (template?.id ?? BLANK_TEMPLATE) : undefined;
  /** Edits this form held when it was last unmounted (Settings opened another section, the panel closed). */
  const draft = draftFor(draftKey, templateKey);
  // svelte-ignore state_referenced_locally
  const start = draft?.saved ?? (provider ? formFrom(provider) : formFromTemplate(template, ai.providers.map((p) => p.name)));
  // svelte-ignore state_referenced_locally
  const startModels = provider?.models ?? template?.models ?? [];

  let form = $state(structuredClone(draft?.form ?? start));
  /** The form as the provider was last loaded or saved: what "unsaved changes" compare with. */
  let saved = $state.raw<ProviderForm>(structuredClone(start));
  /** The user chose "Other model…": type an id instead of picking one. */
  let otherModel = $state(false);
  let showKey = $state(false);
  let busy = $state<'save' | 'test' | 'remove' | null>(null);
  // svelte-ignore state_referenced_locally
  let result = $state<{ ok: boolean; text: string } | null>(notice ? { ok: true, text: notice } : draft ? { ok: true, text: t('aiProviders.draftRestored') } : null);
  /** Submitted once: "empty" errors show from now on. */
  let tried = $state(false);
  let confirmRemove = $state(false);

  const kind = $derived(provider?.kind ?? 'api');
  const editable = $derived(isNew || (provider !== null && isUser(provider)));
  const needsKey = $derived(provider ? provider.needs_key : (template?.needs_key ?? false));
  const models = $derived(provider ? provider.models : startModels);
  const taken = $derived(ai.providers.filter((p) => p.id !== provider?.id).map((p) => p.name));
  const errors = $derived(validate(form, { kind, user: editable, taken }));
  const hasErrors = $derived(Object.keys(errors).length > 0);
  /** Errors to show: "empty" ones wait for the first submit, the rest show while typing. */
  const shown = $derived(Object.fromEntries(Object.entries(errors).filter(([, key]) => tried || !key.endsWith('Empty'))) as typeof errors);
  /** A text field instead of the list: asked for, or the model is not in the list. */
  const typeModel = $derived(!models.length || otherModel || (form.model !== '' && !models.includes(form.model)));
  const headerCount = $derived(form.headers.filter((h) => h.name.trim()).length);
  /** The model used while none is chosen ("Provider default"), when the core told us. */
  const autoModel = $derived(provider && !provider.model_set ? provider.model : null);

  $effect(() => {
    const p = provider;
    if (!p) return;
    const fresh = formFrom(p);
    untrack(() => {
      form = rebase($state.snapshot(form), saved, fresh);
      saved = fresh;
    });
  });

  $effect(() => {
    dirty = isNew ? isDraftDirty(form, start) : provider ? isDirty(form, saved, provider) : false;
  });

  // Unsaved edits also live outside this component, so a remount the list cannot ask about
  // (another Settings section, the AI panel closed) brings them back; see provider-draft.ts.
  $effect(() => {
    if (dirty) keepDraft({ key: draftKey, templateId: templateKey, form: $state.snapshot(form), saved: isNew ? start : saved });
    else dropDraft(draftKey);
  });

  function err(field: keyof typeof errors): string | null {
    const key = shown[field];
    return key ? t(key, { max: MAX_NAME }) : null;
  }

  async function run(what: 'save' | 'test' | 'remove', work: () => Promise<void>) {
    busy = what;
    result = null;
    try {
      await work();
    } catch (e) {
      result = { ok: false, text: (e as Error).message };
    } finally {
      busy = null;
    }
  }

  /** Save pending edits of an existing provider; returns the provider as saved. */
  async function saveEdits(p: AiProvider): Promise<AiProvider> {
    if (!isDirty(form, saved, p)) return p;
    const next = await ai.saveProvider(changes(form, saved, p));
    saved = formFrom(next);
    form = structuredClone(saved);
    return next;
  }

  function submit() {
    tried = true;
    if (hasErrors) return;
    void run('save', async () => {
      if (!provider) {
        const created = await ai.api.call('ai.provider_add', addArgs(form, template?.id ?? null));
        await ai.refreshProviders();
        dirty = false;
        oncreated?.(created);
        return;
      }
      const next = await saveEdits(provider);
      result = { ok: true, text: next.configured ? t('aiProviders.saved') : t('aiProviders.savedNotReady') };
    });
  }

  function describe(r: AiProviderTest, p: AiProvider): string {
    // Known problems in the UI's language; a network or endpoint error as the core reports it.
    if (!r.ok && r.problem) return t(PROBLEM[r.problem], { name: p.name });
    if (!r.ok) return r.error ?? t('aiProviders.result.failed');
    if (r.version) return t('aiProviders.result.okVersion', { version: r.version });
    if (!r.models.length) return t('aiProviders.result.ok');
    const list = r.models.slice(0, 6).join(', ') + (r.models.length > 6 ? '…' : '');
    return tp('aiProviders.result.okModels', r.models.length, { list });
  }

  /** Test what is saved; unsaved edits are saved first (the core tests its stored settings). */
  function test() {
    const p = provider;
    if (!p) return;
    if (dirty) {
      tried = true;
      if (hasErrors) return;
    }
    void run('test', async () => {
      const saved = await saveEdits(p);
      const r = await ai.testProvider(saved.id);
      result = { ok: r.ok, text: describe(r, saved) };
    });
  }

  function removeKey(p: AiProvider) {
    void run('save', async () => {
      const next = await ai.saveProvider({ id: p.id, api_key: '' });
      form.apiKey = '';
      result = { ok: true, text: next.configured ? t('aiProviders.key.removed') : `${t('aiProviders.key.removed')} ${t('aiProviders.key.missing')}` };
    });
  }

  function makeDefault(p: AiProvider) {
    void run('save', async () => {
      await ai.saveProvider({ id: p.id, default: true });
      await ai.refreshProviders(); // the previous default lost its flag
      result = { ok: true, text: t('aiProviders.defaultDone', { name: p.name }) };
    });
  }

  function remove(p: AiProvider) {
    void run('remove', async () => {
      await ai.api.call('ai.provider_remove', { id: p.id });
      // Also moves "new chats" to another provider when this one was selected.
      await ai.refreshProviders();
      dirty = false;
      onremoved?.(p.id);
    });
  }

  function pickModel(value: string) {
    if (value === OTHER_MODEL) otherModel = true;
    else form.model = value;
  }
</script>

<form
  class="form"
  novalidate
  onsubmit={(e) => {
    e.preventDefault();
    submit();
  }}
>
  {#if editable}
    <label>
      <span>{t('aiProviders.field.name')}</span>
      <input spellcheck="false" maxlength={MAX_NAME + 1} bind:value={form.name} aria-invalid={!!err('name')} />
      {#if err('name')}<span class="field-err">{err('name')}</span>{/if}
    </label>
    <label>
      <span>{t('aiProviders.field.api')}</span>
      <select bind:value={form.api}>
        {#each APIS as a (a.id)}<option value={a.id}>{t(a.label)}</option>{/each}
      </select>
    </label>
  {/if}

  {#if kind === 'api'}
    <label>
      <span>{t('aiProviders.field.baseUrl')}</span>
      <input type="url" spellcheck="false" bind:value={form.baseUrl} placeholder="https://…/v1" aria-invalid={!!err('baseUrl')} />
      {#if err('baseUrl')}
        <span class="field-err">{err('baseUrl')}</span>
      {:else if editable}
        <span class="help">{t('aiProviders.field.baseUrlHint', { example: EXAMPLE_URL })}</span>
      {/if}
    </label>
    <label>
      <span>{needsKey ? t('aiProviders.field.key') : t('aiProviders.field.keyOptional')}</span>
      <span class="inline">
        <input
          type={showKey ? 'text' : 'password'}
          autocomplete="off"
          spellcheck="false"
          bind:value={form.apiKey}
          placeholder={provider?.has_key ? t('aiProviders.key.stored') : needsKey ? t('aiProviders.key.paste') : t('aiProviders.key.notNeeded')}
        />
        <button
          type="button"
          class="icon-btn"
          aria-pressed={showKey}
          aria-label={showKey ? t('aiProviders.key.hide') : t('aiProviders.key.show')}
          title={showKey ? t('aiProviders.key.hide') : t('aiProviders.key.show')}
          onclick={() => (showKey = !showKey)}
        >
          <Icon name={showKey ? 'eye-off' : 'eye'} size={14} />
        </button>
        {#if provider?.has_key}
          <button type="button" class="btn" disabled={busy !== null} onclick={() => removeKey(provider)}>{t('aiProviders.key.remove')}</button>
        {/if}
      </span>
      {#if provider && needsKey && !provider.has_key && !form.apiKey.trim()}
        <span class="help warn">{t('aiProviders.key.missing')}</span>
      {/if}
    </label>
  {:else}
    <label>
      <span>{t('aiProviders.field.program')}</span>
      <input spellcheck="false" bind:value={form.path} placeholder={t('aiProviders.field.programPlaceholder', { name: provider?.id === 'codex' ? 'codex' : 'claude' })} />
    </label>
  {/if}

  <label>
    <span>{t('aiProviders.field.model')}</span>
    {#if !typeModel}
      <select value={form.model} onchange={(e) => pickModel(e.currentTarget.value)}>
        <option value="">{autoModel ? t('aiProviders.model.autoWith', { model: autoModel }) : t('aiProviders.model.auto')}</option>
        {#each models as m (m)}<option value={m}>{m}</option>{/each}
        <option value={OTHER_MODEL}>{t('aiProviders.model.other')}</option>
      </select>
    {:else}
      <span class="inline">
        <input spellcheck="false" bind:value={form.model} placeholder={autoModel ?? models[0] ?? t('aiProviders.model.placeholder')} />
        {#if models.length}
          <button
            type="button"
            class="btn"
            onclick={() => {
              otherModel = false;
              if (!models.includes(form.model)) form.model = '';
            }}>{t('aiProviders.model.pick')}</button
          >
        {/if}
      </span>
    {/if}
    {#if kind === 'api'}
      <span class="help">{models.length && provider ? tp('aiProviders.model.count', models.length) : t('aiProviders.model.testHint')}</span>
    {/if}
  </label>

  {#if kind === 'api'}
    <details class="headers" open={form.headers.length > 0}>
      <summary>{headerCount ? t('aiProviders.headers.titleCount', { n: headerCount }) : t('aiProviders.headers.title')}</summary>
      <div class="headers-body">
      <p class="help">{t('aiProviders.headers.note')}</p>
      {#each form.headers as h, i (i)}
        <div class="header-row">
          {#if h.stored}
            <!-- The saved value belongs to this name, which therefore stays fixed. -->
            <input spellcheck="false" aria-label={t('aiProviders.headers.name')} title={t('aiProviders.headers.storedName')} readonly value={h.name} />
            <input
              spellcheck="false"
              autocomplete="off"
              aria-label={t('aiProviders.headers.newValue', { name: h.name })}
              placeholder={t('aiProviders.headers.stored')}
              bind:value={h.value}
            />
          {:else}
            <input spellcheck="false" aria-label={t('aiProviders.headers.name')} placeholder={t('aiProviders.headers.name')} bind:value={h.name} />
            <input spellcheck="false" aria-label={t('aiProviders.headers.value')} placeholder={t('aiProviders.headers.value')} bind:value={h.value} />
          {/if}
          <button type="button" class="icon-btn" aria-label={t('aiProviders.headers.remove')} title={t('aiProviders.headers.remove')} onclick={() => form.headers.splice(i, 1)}>
            <Icon name="x" size={13} />
          </button>
        </div>
      {/each}
      {#if err('headers')}<p class="field-err">{err('headers')}</p>{/if}
      <button type="button" class="btn" onclick={() => form.headers.push({ name: '', value: '' })}>
        <Icon name="plus" size={13} />
        {t('aiProviders.headers.add')}
      </button>
      </div>
    </details>
  {/if}

  <p class="note">{t('aiProviders.note')}</p>

  {#if isNew}
    <div class="actions">
      <button class="btn primary" type="submit" disabled={busy !== null}>{busy === 'save' ? t('aiProviders.new.adding') : t('aiProviders.new.submit')}</button>
      <button class="btn" type="button" disabled={busy !== null} onclick={() => oncancel?.()}>{t('common.cancel')}</button>
    </div>
  {:else if provider}
    <div class="actions">
      <button class="btn primary" type="submit" disabled={busy !== null || !dirty}>{busy === 'save' ? t('aiProviders.saving') : t('common.save')}</button>
      <button class="btn" type="button" disabled={busy !== null} onclick={test}>
        {busy === 'test' ? t('aiProviders.testing') : dirty ? t('aiProviders.saveAndTest') : t('common.test')}
      </button>
      <button class="btn" type="button" disabled={ai.provider === provider.id} onclick={() => ai.setProvider(provider.id)}>
        {ai.provider === provider.id ? t('aiProviders.inUse') : t('aiProviders.use')}
      </button>
      <button class="btn" type="button" disabled={busy !== null || !!provider.is_default} title={t('aiProviders.defaultTitle')} onclick={() => makeDefault(provider)}>
        {provider.is_default ? t('aiProviders.isDefault') : t('aiProviders.makeDefault')}
      </button>
      {#if editable && !confirmRemove}
        <button class="btn danger" type="button" disabled={busy !== null} onclick={() => (confirmRemove = true)}>
          <Icon name="trash" size={13} />
          {t('common.delete')}
        </button>
      {/if}
    </div>
    {#if editable && confirmRemove}
      <div class="confirm" role="group" aria-label={t('aiProviders.deleteConfirm', { name: provider.name })}>
        <span>{t('aiProviders.deleteConfirm', { name: provider.name })}</span>
        <button class="btn danger" type="button" disabled={busy !== null} onclick={() => remove(provider)}>{t('common.delete')}</button>
        <button class="btn" type="button" disabled={busy !== null} onclick={() => (confirmRemove = false)}>{t('common.cancel')}</button>
      </div>
    {/if}
  {/if}
  {#if result && result.text}
    <p class="result" class:bad={!result.ok} role="status">{result.text}</p>
  {/if}
</form>

<style>
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
  .inline {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  input,
  select {
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
  select {
    padding-right: 4px;
  }
  input:focus,
  select:focus {
    outline: none;
    border-color: var(--accent-line);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  input[aria-invalid='true'] {
    border-color: var(--danger);
  }
  .help,
  .note {
    margin: 0;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    font-weight: 400;
    line-height: 1.45;
  }
  .help.warn {
    color: var(--warn, var(--danger));
  }
  .field-err {
    margin: 0;
    color: var(--danger);
    font-size: var(--fs-xs);
    font-weight: 500;
  }
  .headers {
    font-size: var(--fs-xs);
  }
  .headers summary {
    cursor: pointer;
    color: var(--fg-muted);
    font-weight: 600;
  }
  .headers-body {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 6px;
  }
  .header-row {
    display: flex;
    gap: 6px;
  }
  .headers .btn {
    align-self: flex-start;
  }
  .actions,
  .confirm {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .confirm span {
    font-size: var(--fs-sm);
  }
  .btn.danger {
    color: var(--danger);
  }
  .btn {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .result {
    margin: 0;
    color: var(--ok);
    font-size: var(--fs-xs);
    overflow-wrap: anywhere;
  }
  .result.bad {
    color: var(--danger);
  }
</style>
