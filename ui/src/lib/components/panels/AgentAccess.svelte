<script lang="ts">
  import type { AiAgentAccess, AiCodexApproval, AiCodexSandbox, AiPermissionMode, AiProvider } from '../../api';
  import type { AiStore } from '../../ai/store.svelte';
  import { t } from '../../i18n/index.svelte';

  /**
   * What the CLI agents (Claude Code, Codex) may do on this computer in new chats: permission
   * mode, sandbox and approvals, working folder and extra folders (`ai.agent_access_set`).
   * Every new chat can still override them in the AI panel.
   */
  let { store }: { store: AiStore } = $props();

  const MODES: AiPermissionMode[] = ['ask', 'accept_edits', 'plan', 'full_access', 'browser_only'];
  const SANDBOXES: AiCodexSandbox[] = ['read_only', 'workspace_write', 'full_access'];
  const APPROVALS: AiCodexApproval[] = ['untrusted', 'on_request', 'never'];

  interface Form {
    permission_mode: AiPermissionMode;
    sandbox: AiCodexSandbox;
    approval_policy: AiCodexApproval;
    cwd: string;
    add_dirs: string;
  }

  const clis = $derived(store.providers.filter((p) => p.kind === 'cli'));
  let forms = $state<Record<string, Form>>({});
  let busy = $state<string | null>(null);
  let results = $state<Record<string, { ok: boolean; text: string }>>({});

  function toForm(a: AiAgentAccess): Form {
    return {
      permission_mode: a.permission_mode,
      sandbox: a.sandbox,
      approval_policy: a.approval_policy,
      cwd: a.cwd ?? '',
      add_dirs: a.add_dirs.join('\n'),
    };
  }

  $effect(() => {
    for (const p of clis) {
      if (forms[p.id]) continue;
      void store.loadAccess(p.id).then((a) => {
        if (a && !forms[p.id]) forms[p.id] = toForm(a);
      });
    }
  });

  async function save(p: AiProvider) {
    const form = forms[p.id];
    if (!form) return;
    busy = p.id;
    delete results[p.id];
    try {
      const codex = p.protocol === 'codex';
      const saved = await store.saveAccess({
        id: p.id,
        cwd: form.cwd.trim(),
        ...(codex
          ? { sandbox: form.sandbox, approval_policy: form.approval_policy }
          : {
              permission_mode: form.permission_mode,
              add_dirs: form.add_dirs
                .split('\n')
                .map((d) => d.trim())
                .filter(Boolean),
            }),
      });
      forms[p.id] = toForm(saved);
      results[p.id] = { ok: true, text: t('aiChat.accessSettings.saved') };
    } catch (err) {
      results[p.id] = { ok: false, text: (err as Error).message };
    } finally {
      busy = null;
    }
  }
</script>

{#if clis.length}
  <section class="access">
    <h4>{t('aiChat.accessSettings.title')}</h4>
    <p class="note">{t('aiChat.accessSettings.intro')}</p>
    {#each clis as p (p.id)}
      {@const form = forms[p.id]}
      {@const defaults = store.access[p.id]}
      <form
        class="agent"
        onsubmit={(e) => {
          e.preventDefault();
          void save(p);
        }}
      >
        <h5>{p.name}</h5>
        {#if !form}
          <p class="note">{t('common.loading')}</p>
        {:else}
          {#if p.protocol === 'codex'}
            <label>
              <span>{t('aiChat.newChat.sandbox')}</span>
              <select bind:value={form.sandbox}>
                {#each SANDBOXES as s (s)}<option value={s}>{t(`aiChat.sandbox.${s}`)}</option>{/each}
              </select>
            </label>
            <label>
              <span>{t('aiChat.accessSettings.approvals')}</span>
              <select bind:value={form.approval_policy}>
                {#each APPROVALS as a (a)}<option value={a}>{t(`aiChat.approval.${a}`)}</option>{/each}
              </select>
            </label>
            {#if form.sandbox === 'full_access'}<p class="warn">{t('aiChat.accessSettings.fullWarn')}</p>{/if}
          {:else}
            <label>
              <span>{t('aiChat.newChat.permissions')}</span>
              <select bind:value={form.permission_mode}>
                {#each MODES as m (m)}<option value={m}>{t(`aiChat.mode.${m}`)}</option>{/each}
              </select>
            </label>
            <p class="note" class:warn={form.permission_mode === 'full_access'}>{t(`aiChat.modeHint.${form.permission_mode}`)}</p>
          {/if}
          <label>
            <span>{t('aiChat.accessSettings.cwd')}</span>
            <input spellcheck="false" bind:value={form.cwd} placeholder={defaults?.default_cwd ?? ''} />
          </label>
          <p class="note">{t('aiChat.accessSettings.cwdHint', { path: defaults?.default_cwd ?? '' })}</p>
          {#if p.protocol !== 'codex'}
            <label>
              <span>{t('aiChat.accessSettings.addDirs')}</span>
              <textarea rows="2" spellcheck="false" bind:value={form.add_dirs} placeholder={t('aiChat.accessSettings.addDirsPlaceholder')}></textarea>
            </label>
          {/if}
          <div class="actions">
            <button class="btn primary" type="submit" disabled={busy !== null}>{busy === p.id ? t('aiChat.accessSettings.saving') : t('common.save')}</button>
          </div>
          {#if results[p.id]}
            <p class="result" class:bad={!results[p.id].ok} role="status">{results[p.id].text}</p>
          {/if}
        {/if}
      </form>
    {/each}
  </section>
{/if}

<style>
  .access {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin-top: 10px;
    padding: 10px 12px 12px;
    border-top: 1px solid var(--border);
  }
  h4 {
    margin: 0;
    font-size: var(--fs-sm);
    font-weight: 650;
  }
  h5 {
    margin: 0;
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .agent {
    display: flex;
    flex-direction: column;
    gap: 7px;
    padding: 9px 10px 10px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  input,
  select,
  textarea {
    min-width: 0;
    padding: 0 9px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--field-focus);
    color: var(--fg);
    font-size: var(--fs-sm);
    font-weight: 400;
    user-select: text;
  }
  input,
  select {
    height: 28px;
  }
  textarea {
    padding: 5px 9px;
    resize: vertical;
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
  }
  input:focus,
  select:focus,
  textarea:focus {
    outline: none;
    border-color: var(--accent-line);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .note,
  .warn {
    margin: 0;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }
  .warn,
  .note.warn {
    color: var(--danger);
  }
  .actions {
    display: flex;
    gap: 6px;
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
