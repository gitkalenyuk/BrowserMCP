<script lang="ts">
  import type { Snippet } from 'svelte';

  // A modal <dialog> (in-page: CEF Alloy has no UI for window.confirm/prompt).
  let {
    open = $bindable(false),
    title,
    children,
    actions,
    onsubmit,
    wide = false,
  }: {
    open?: boolean;
    title: string;
    children: Snippet;
    actions: Snippet;
    /** Enter / the submit button. */
    onsubmit?: () => void;
    wide?: boolean;
  } = $props();

  let el: HTMLDialogElement;

  $effect(() => {
    if (open && !el.open) el.showModal();
    else if (!open && el.open) el.close();
  });
</script>

<dialog bind:this={el} class:wide onclose={() => (open = false)} aria-labelledby="dlg-title">
  <form
    method="dialog"
    onsubmit={(e) => {
      e.preventDefault();
      onsubmit?.();
    }}
  >
    <h2 id="dlg-title">{title}</h2>
    <div class="body">{@render children()}</div>
    <div class="actions">{@render actions()}</div>
  </form>
</dialog>

<style>
  dialog {
    width: min(440px, calc(100vw - 32px));
    padding: 0;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--surface);
    color: var(--fg);
    box-shadow: var(--shadow-pop);
  }
  dialog.wide {
    width: min(560px, calc(100vw - 32px));
  }
  dialog::backdrop {
    background: var(--scrim);
  }
  form {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 18px 18px 16px;
  }
  h2 {
    margin: 0;
    font-size: var(--fs-lg);
    font-weight: 650;
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 10px;
    color: var(--fg-muted);
    overflow-wrap: anywhere;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
