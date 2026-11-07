<script lang="ts">
  // One user or assistant message of the AI chat: selectable text, hover actions (time,
  // Copy, and for user messages Edit / Retry), the inline editor of a user message, and
  // assistant text as Markdown.
  import type { ChatItem } from '../../ai/thread';
  import { formatWhen } from '../../i18n/intl';
  import { t } from '../../i18n/index.svelte';
  import Icon from '../Icon.svelte';
  import AiCopyButton from './AiCopyButton.svelte';
  import AiMarkdown from './AiMarkdown.svelte';

  let {
    item,
    editable = false,
    editBlocked = null,
    retry = false,
    editing = false,
    onstartedit,
    oncanceledit,
    onsave,
    onretry,
    onlink,
    oncopyfail,
  }: {
    item: Extract<ChatItem, { kind: 'user' | 'assistant' }>;
    /** A user message of the conversation (not a queued one). */
    editable?: boolean;
    /** Why editing is not possible now (the agent works), shown on the pencil. */
    editBlocked?: string | null;
    /** The last user message: offers Retry. */
    retry?: boolean;
    editing?: boolean;
    onstartedit?: () => void;
    oncanceledit?: () => void;
    onsave?: (text: string) => void;
    onretry?: () => void;
    onlink: (href: string) => void;
    oncopyfail?: (message: string) => void;
  } = $props();

  let text = $state('');
  let area = $state<HTMLTextAreaElement>();

  $effect(() => {
    if (!editing || !area) return;
    text = item.text;
    const el = area;
    queueMicrotask(() => {
      el.focus();
      el.setSelectionRange(el.value.length, el.value.length);
      grow(el);
    });
  });

  function grow(el: HTMLTextAreaElement) {
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight + 2, 260)}px`;
  }

  function save() {
    const next = text.trim();
    if (next) onsave?.(next);
  }

  const time = $derived(item.ts ? formatWhen(item.ts) : null);
</script>

{#if item.kind === 'user'}
  <div class="row user" class:queued={item.queued} class:editing>
    {#if editing}
      <div class="editor">
        <label class="sr-only" for="ai-edit">{t('aiChat.msg.editLabel')}</label>
        <textarea
          id="ai-edit"
          bind:this={area}
          bind:value={text}
          rows="2"
          oninput={(e) => grow(e.currentTarget)}
          onkeydown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
              e.preventDefault();
              save();
            } else if (e.key === 'Escape') {
              e.preventDefault();
              e.stopPropagation();
              oncanceledit?.();
            }
          }}
        ></textarea>
        <div class="edit-bar">
          <span class="hint">{t('aiChat.msg.editHint')}</span>
          <button class="btn" type="button" onclick={() => oncanceledit?.()}>{t('common.cancel')}</button>
          <button class="btn primary" type="button" disabled={!text.trim()} onclick={save}>{t('aiChat.msg.save')}</button>
        </div>
      </div>
    {:else}
      <div class="bubble">{item.text}</div>
      <div class="actions" role="toolbar" aria-label={t('aiChat.msg.actions')}>
        {#if item.queued}
          <span class="pill" title={t('aiChat.queuedTitle')}><Icon name="clock" size={10} />{t('aiChat.queued')}</span>
        {/if}
        {#if time}<time class="time" title={t('aiChat.msg.sentAt', { time })}>{time}</time>{/if}
        <AiCopyButton text={item.text} label={t('aiChat.msg.copy')} onfail={oncopyfail} />
        {#if editable}
          <button
            class="act"
            type="button"
            aria-label={t('aiChat.msg.edit')}
            title={editBlocked ?? t('aiChat.msg.editTitle')}
            aria-disabled={editBlocked ? 'true' : undefined}
            onclick={() => !editBlocked && onstartedit?.()}
          >
            <Icon name="pencil" size={12} />
          </button>
        {/if}
        {#if retry}
          <button
            class="act"
            type="button"
            aria-label={t('aiChat.msg.retry')}
            title={editBlocked ?? t('aiChat.msg.retryTitle')}
            aria-disabled={editBlocked ? 'true' : undefined}
            onclick={() => !editBlocked && onretry?.()}
          >
            <Icon name="reload" size={12} />
          </button>
        {/if}
      </div>
    {/if}
  </div>
{:else}
  <div class="row assistant" class:streaming={item.streaming}>
    <AiMarkdown text={item.text} {onlink} {oncopyfail} />
    {#if !item.streaming}
      <div class="actions" role="toolbar" aria-label={t('aiChat.msg.actions')}>
        <AiCopyButton text={item.text} label={t('aiChat.msg.copy')} onfail={oncopyfail} />
        {#if time}<time class="time">{time}</time>{/if}
      </div>
    {/if}
  </div>
{/if}

<style>
  .row {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-width: 100%;
    user-select: text;
  }
  .row.user {
    align-self: flex-end;
    align-items: flex-end;
    max-width: 92%;
  }
  .row.user.editing {
    align-self: stretch;
    max-width: 100%;
  }
  .row.assistant {
    align-self: stretch;
    padding: 1px 2px;
    color: var(--fg);
  }
  .bubble {
    padding: 7px 10px;
    border-radius: var(--r-lg) var(--r-lg) 3px var(--r-lg);
    background: var(--accent);
    color: var(--on-accent);
    font-size: var(--fs-sm);
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .bubble::selection {
    background: color-mix(in oklab, var(--on-accent) 35%, transparent);
  }
  .queued .bubble {
    background: color-mix(in oklab, var(--accent) 55%, var(--surface));
    border: 1px dashed var(--accent-line);
  }
  .streaming :global(.md > :last-child)::after {
    content: '';
    display: inline-block;
    width: 7px;
    height: 13px;
    margin-left: 2px;
    vertical-align: -2px;
    background: var(--accent);
    animation: blink 1s steps(1) infinite;
  }
  @keyframes blink {
    50% {
      opacity: 0;
    }
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 1px;
    min-height: 22px;
    opacity: 0;
    transition: opacity 120ms;
  }
  .row:hover .actions,
  .row:focus-within .actions,
  .queued .actions {
    opacity: 1;
  }
  .queued .actions > :not(.pill) {
    display: none;
  }
  .time {
    padding: 0 4px;
    color: var(--fg-subtle);
    font-size: 10px;
    font-variant-numeric: tabular-nums;
  }
  .act {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border: 0;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--fg-subtle);
  }
  .act:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .act[aria-disabled='true'] {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 0 6px;
    height: 16px;
    border-radius: 8px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 10px;
    font-weight: 650;
  }
  .editor {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px;
    border: 1px solid var(--accent-line);
    border-radius: var(--r-lg);
    background: var(--surface);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .editor textarea {
    width: 100%;
    min-height: 44px;
    padding: 2px 4px;
    border: 0;
    background: transparent;
    color: var(--fg);
    font-size: var(--fs-sm);
    line-height: 1.5;
    resize: none;
    outline: none;
    user-select: text;
  }
  .edit-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: 6px;
  }
  .hint {
    flex: 1 1 100%;
    min-width: 0;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    line-height: 1.35;
  }
  .btn {
    height: 24px;
    padding: 0 9px;
    font-size: var(--fs-xs);
  }
  .btn.primary {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--on-accent);
  }
</style>
