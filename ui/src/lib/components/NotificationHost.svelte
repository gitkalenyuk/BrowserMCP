<script lang="ts">
  // Toasts from the core (crates/notify): `ui.notification` shows one, `ui.notification_closed`
  // hides it (answered here or on Telegram, expired, resolved elsewhere). Buttons answer with
  // `ui.answer`. Rendered as bars in the chrome's own layout (between toolbar and page), because
  // the page is a separate native view that would cover anything floating above it.
  import { onDestroy } from 'svelte';
  import { t, tp } from '../i18n/index.svelte';
  import { addToast, expire, removeToast, visibleToasts, type Toast } from '../notifications';
  import { useChrome } from '../state/chrome.svelte';
  import Icon from './Icon.svelte';

  const chrome = useChrome();
  let toasts = $state<Toast[]>([]);
  let busy = $state<string | null>(null);
  const view = $derived(visibleToasts(toasts));

  const offs = [
    chrome.api.on('ui.notification', (n) => (toasts = addToast(toasts, n, Date.now()))),
    chrome.api.on('ui.notification_closed', (p) => (toasts = removeToast(toasts, p.id))),
  ];
  const timer = setInterval(() => (toasts = expire(toasts, Date.now())), 1000);
  onDestroy(() => {
    offs.forEach((off) => off());
    clearInterval(timer);
  });

  async function answer(toast: Toast, choice: string) {
    busy = toast.id;
    try {
      await chrome.api.call('ui.answer', { id: toast.id, choice });
    } catch (err) {
      // Already answered elsewhere / expired: just drop the toast.
      console.warn('[notify] answer failed', err);
    } finally {
      busy = null;
      toasts = removeToast(toasts, toast.id);
    }
  }
</script>

{#if view.shown.length}
  <div class="host" role="region" aria-label={t('components.notifications')}>
    {#each view.shown as n (n.id)}
      <div class="note {n.level}" role={n.actions.length ? 'alertdialog' : 'status'}>
        <span class="dot" aria-hidden="true"></span>
        <span class="text">
          <span class="title">{n.title}</span>
          {#if n.body}<span class="body">{n.body}</span>{/if}
        </span>
        {#each n.actions as a (a.id)}
          <button class="btn" disabled={busy === n.id} onclick={() => answer(n, a.id)}>{a.label}</button>
        {/each}
        {#if !n.actions.length}
          <button class="icon-btn" aria-label={t('chrome.dismiss')} onclick={() => (toasts = removeToast(toasts, n.id))}>
            <Icon name="x" size={14} />
          </button>
        {/if}
      </div>
    {/each}
    {#if view.hidden}
      <div class="more">{tp('components.notifications.more', view.hidden)}</div>
    {/if}
  </div>
{/if}

<style>
  .host {
    display: flex;
    flex-direction: column;
    border-bottom: 1px solid var(--border);
    background: var(--toolbar);
  }
  .note {
    --tone: var(--accent);
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 34px;
    padding: 3px 6px 3px 14px;
    font-size: var(--fs-sm);
    background: color-mix(in oklab, var(--tone) 8%, var(--toolbar));
  }
  .note + .note {
    border-top: 1px solid var(--border);
  }
  .note.success {
    --tone: var(--ok);
  }
  .note.warning {
    --tone: var(--warn);
  }
  .note.error {
    --tone: var(--danger);
  }
  .dot {
    width: 8px;
    height: 8px;
    flex: none;
    border-radius: 50%;
    background: var(--tone);
  }
  .text {
    display: flex;
    gap: 8px;
    flex: 1;
    min-width: 0;
    white-space: nowrap;
    overflow: hidden;
  }
  .title {
    font-weight: 650;
    color: var(--fg);
  }
  .body {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--fg-muted);
  }
  .btn {
    height: 26px;
    flex: none;
  }
  .more {
    padding: 2px 14px 4px;
    font-size: var(--fs-xs);
    color: var(--fg-subtle);
  }
</style>
