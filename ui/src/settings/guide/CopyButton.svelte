<script lang="ts">
  import Icon from '../../lib/components/Icon.svelte';
  import { t } from '../../lib/i18n/index.svelte';

  let { text, onfail }: { text: string; onfail: (message: string) => void } = $props();

  let copied = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      copied = true;
      clearTimeout(timer);
      timer = setTimeout(() => (copied = false), 1400);
    } catch {
      onfail(t('guide.copyFailed'));
    }
  }
</script>

<button class="btn copy" onclick={copy} aria-live="polite">
  <Icon name={copied ? 'check' : 'copy'} size={13} />
  <span>{copied ? t('common.copied') : t('common.copy')}</span>
</button>

<style>
  .copy {
    height: 24px;
    padding: 0 8px;
    font-size: var(--fs-xs);
  }
</style>
