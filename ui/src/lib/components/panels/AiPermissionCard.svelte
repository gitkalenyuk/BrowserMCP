<script lang="ts">
  import type { AiPermission, AiPermissionDecision } from '../../api';
  import { t } from '../../i18n/index.svelte';
  import Icon from '../Icon.svelte';

  /**
   * One approval request of a CLI agent in the chat. Pending: what would run, why, and the
   * answers (allow once / always allow this tool in this chat / deny with an optional reason).
   * Answered: a one-line note of the outcome.
   */
  let {
    request,
    agent,
    onanswer,
  }: {
    request: AiPermission;
    /** Display name of the agent ("Claude Code #2"). */
    agent: string;
    onanswer: (decision: AiPermissionDecision, reason?: string) => Promise<void>;
  } = $props();

  let denying = $state(false);
  let reason = $state('');
  let busy = $state(false);
  let error = $state<string | null>(null);

  const tool = $derived(toolName(request.tool));
  const heading = $derived(
    request.kind === 'command'
      ? t('aiChat.perm.command', { agent })
      : request.kind === 'file_change'
        ? t('aiChat.perm.fileChange', { agent })
        : t('aiChat.perm.wants', { agent, tool }),
  );
  const details = $derived(describe(request));

  function toolName(name: string): string {
    if (name === 'file_change') return t('aiChat.tool.fileChange');
    return name.split('__').pop() || name;
  }

  /** The most telling part of the input: a command, file list, URL, or the whole JSON. */
  function describe(r: AiPermission): { label: string; text: string } | null {
    const input = (r.input ?? {}) as Record<string, unknown>;
    if (typeof input.command === 'string') return { label: t('aiChat.perm.commandLabel'), text: input.command };
    if (Array.isArray(input.files) && input.files.length) return { label: t('aiChat.perm.files'), text: input.files.join('\n') };
    if (typeof input.file_path === 'string') return { label: t('aiChat.perm.file'), text: input.file_path };
    if (typeof input.url === 'string') return { label: 'URL', text: input.url };
    const json = JSON.stringify(r.input, null, 2);
    return json && json !== '{}' && json !== 'null' ? { label: t('aiChat.tool.args'), text: json } : null;
  }

  async function answer(decision: AiPermissionDecision) {
    busy = true;
    error = null;
    try {
      await onanswer(decision, decision === 'deny' ? reason : undefined);
    } catch (err) {
      error = (err as Error).message;
    } finally {
      busy = false;
    }
  }

  function outcome(r: AiPermission): string {
    switch (r.status) {
      case 'allowed':
        return r.always ? t('aiChat.perm.allowedAlways', { tool }) : t('aiChat.perm.allowed', { tool });
      case 'denied':
        return r.answer ? t('aiChat.perm.deniedWhy', { tool, reason: r.answer }) : t('aiChat.perm.denied', { tool });
      default:
        return t('aiChat.perm.cancelled', { tool });
    }
  }
</script>

{#if request.status === 'pending'}
  <div class="card" role="group" aria-label={heading}>
    <div class="head">
      <span class="glyph"><Icon name="hand" size={14} /></span>
      <strong>{heading}</strong>
    </div>
    {#if request.title && request.title !== heading}<p class="title">{request.title}</p>{/if}
    {#if details}
      <div class="k">{details.label}</div>
      <pre>{details.text}</pre>
    {/if}
    {#if typeof (request.input as Record<string, unknown> | null)?.cwd === 'string'}
      <p class="muted">{t('aiChat.perm.folder', { cwd: String((request.input as Record<string, unknown>).cwd) })}</p>
    {/if}
    {#if request.reason}<p class="muted">{t('aiChat.perm.reason', { reason: request.reason })}</p>{/if}
    {#if denying}
      <form
        class="deny"
        onsubmit={(e) => {
          e.preventDefault();
          void answer('deny');
        }}
      >
        <!-- svelte-ignore a11y_autofocus -->
        <input bind:value={reason} placeholder={t('aiChat.perm.denyReason')} aria-label={t('aiChat.perm.denyReason')} autofocus />
        <div class="actions">
          <button class="btn danger" type="submit" disabled={busy}>{t('aiChat.perm.denySend')}</button>
          <button class="btn" type="button" disabled={busy} onclick={() => (denying = false)}>{t('common.cancel')}</button>
        </div>
      </form>
    {:else}
      <div class="actions">
        <button class="btn primary" disabled={busy} onclick={() => answer('allow')}>{t('aiChat.perm.allowOnce')}</button>
        <button class="btn" disabled={busy} onclick={() => answer('allow_always')}>{t('aiChat.perm.allowAlways', { tool })}</button>
        <button class="btn" disabled={busy} onclick={() => (denying = true)}>{t('aiChat.perm.deny')}</button>
      </div>
    {/if}
    {#if error}<p class="err" role="alert">{error}</p>{/if}
  </div>
{:else}
  <div class="note" class:bad={request.status === 'denied'}>
    <Icon name={request.status === 'allowed' ? 'check' : request.status === 'denied' ? 'x' : 'info'} size={12} stroke={2.25} />
    <span>{outcome(request)}</span>
  </div>
{/if}

<style>
  .card {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 9px 10px 10px;
    border: 1px solid var(--accent-line);
    border-radius: var(--r);
    background: var(--accent-soft);
    font-size: var(--fs-sm);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .glyph {
    display: grid;
    place-items: center;
    color: var(--accent);
  }
  strong {
    font-weight: 650;
  }
  .title,
  .muted {
    margin: 0;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }
  .k {
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  pre {
    margin: 0;
    max-height: 180px;
    padding: 6px 8px;
    overflow: auto;
    border-radius: var(--r-sm);
    background: var(--surface);
    font: var(--fs-xs) / 1.45 var(--font-mono);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 2px;
  }
  .deny {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  input {
    height: 28px;
    padding: 0 9px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--surface);
    color: var(--fg);
    font-size: var(--fs-sm);
    user-select: text;
  }
  input:focus {
    outline: none;
    border-color: var(--accent-line);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .btn.danger {
    border-color: color-mix(in oklab, var(--danger) 40%, transparent);
    color: var(--danger);
  }
  .err {
    margin: 0;
    color: var(--danger);
    font-size: var(--fs-xs);
  }
  .note {
    display: flex;
    align-items: center;
    gap: 6px;
    align-self: flex-start;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    overflow-wrap: anywhere;
  }
  .note.bad {
    color: var(--danger);
  }
  .note :global(svg) {
    flex: none;
  }
</style>
