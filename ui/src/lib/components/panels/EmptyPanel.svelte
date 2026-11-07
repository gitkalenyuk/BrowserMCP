<script lang="ts">
  import { t } from '../../i18n/index.svelte';
  import type { Snippet } from 'svelte';
  import Icon, { type IconName } from '../Icon.svelte';

  let {
    icon,
    title,
    text,
    action,
    children,
  }: { icon: IconName; title: string; text: string; action?: string; children?: Snippet } = $props();
</script>

{@render children?.()}
<div class="empty">
  <span class="glyph"><Icon name={icon} size={20} /></span>
  <h3>{title}</h3>
  <p>{text}</p>
  {#if action}
    <button class="btn" disabled title={t('panels.empty.notYet')}>{action}</button>
    <span class="soon">{t('panels.empty.notWired')}</span>
  {/if}
</div>

<style>
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    flex: 1;
    gap: 8px;
    padding: 32px 28px 48px;
    text-align: center;
  }
  .glyph {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    margin-bottom: 4px;
    border-radius: 12px;
    background: var(--sunken);
    color: var(--fg-muted);
  }
  h3 {
    margin: 0;
    font-size: var(--fs);
    font-weight: 650;
  }
  p {
    max-width: 38ch;
    margin: 0 0 8px;
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    line-height: 1.5;
    text-wrap: pretty;
  }
  .soon {
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
</style>
