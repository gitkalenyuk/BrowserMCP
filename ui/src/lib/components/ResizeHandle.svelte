<script lang="ts">
  let {
    edge,
    value,
    min,
    max,
    onchange,
  }: {
    /** Which edge of the parent the handle sits on. Dragging away from the parent grows it. */
    edge: 'left' | 'right';
    value: number;
    min: number;
    max: number;
    onchange: (value: number) => void;
  } = $props();

  let dragging = $state(false);

  function down(e: PointerEvent) {
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    dragging = true;
    const startX = e.clientX;
    const start = value;
    const move = (ev: PointerEvent) => {
      const dx = (ev.clientX - startX) * (edge === 'right' ? 1 : -1);
      onchange(Math.round(Math.min(max, Math.max(min, start + dx))));
    };
    const up = () => {
      dragging = false;
      el.removeEventListener('pointermove', move);
      el.removeEventListener('pointerup', up);
      el.removeEventListener('pointercancel', up);
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
    el.addEventListener('pointercancel', up);
  }

  function key(e: KeyboardEvent) {
    const step = e.shiftKey ? 40 : 10;
    const grow = edge === 'right' ? 'ArrowRight' : 'ArrowLeft';
    const shrink = edge === 'right' ? 'ArrowLeft' : 'ArrowRight';
    if (e.key === grow) onchange(Math.min(max, value + step));
    else if (e.key === shrink) onchange(Math.max(min, value - step));
    else return;
    e.preventDefault();
  }
</script>

<!-- A focusable separator is an interactive widget per WAI-ARIA (window splitter pattern). -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="handle {edge}"
  class:dragging
  role="separator"
  aria-orientation="vertical"
  aria-valuenow={value}
  aria-valuemin={min}
  aria-valuemax={max}
  aria-label="Resize"
  tabindex="0"
  onpointerdown={down}
  onkeydown={key}
></div>

<style>
  .handle {
    position: absolute;
    top: 0;
    bottom: 0;
    z-index: 5;
    width: 7px;
    cursor: col-resize;
    touch-action: none;
  }
  .handle.right {
    right: -4px;
  }
  .handle.left {
    left: -4px;
  }
  .handle::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 3px;
    width: 1px;
    background: transparent;
    transition: background-color 120ms;
  }
  .handle:hover::after,
  .handle:focus-visible::after,
  .handle.dragging::after {
    background: var(--accent);
  }
  .handle:focus-visible {
    outline: none;
  }
</style>
