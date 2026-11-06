<script lang="ts">
  /** An on/off switch (role="switch"). */
  let {
    checked,
    label,
    disabled = false,
    onchange,
  }: { checked: boolean; label: string; disabled?: boolean; onchange: (on: boolean) => void } = $props();
</script>

<button
  type="button"
  class="sw"
  role="switch"
  aria-checked={checked}
  aria-label={label}
  {disabled}
  onclick={() => onchange(!checked)}
>
  <span class="knob"></span>
</button>

<style>
  .sw {
    position: relative;
    flex: none;
    width: 34px;
    height: 20px;
    padding: 0;
    border: 0;
    border-radius: 10px;
    background: var(--border-strong);
    transition: background 0.15s var(--ease-out);
  }
  .sw[aria-checked='true'] {
    background: var(--accent);
  }
  .sw:disabled {
    opacity: 0.5;
  }
  .sw:focus-visible {
    outline: 2px solid var(--accent-line);
    outline-offset: 2px;
  }
  .knob {
    position: absolute;
    top: 2px;
    left: 2px;
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 2px rgb(0 0 0 / 0.3);
    transition: transform 0.15s var(--ease-out);
  }
  .sw[aria-checked='true'] .knob {
    transform: translateX(14px);
  }
</style>
