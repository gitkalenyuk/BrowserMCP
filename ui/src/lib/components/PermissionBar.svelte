<script lang="ts">
  import { t } from '../i18n/index.svelte';
  import { useChrome } from '../state/chrome.svelte';
  import { permissionText } from '../state/workspaces';
  import Icon from './Icon.svelte';

  // A page asks for a permission (geolocation, notifications, camera/mic, clipboard…) and no
  // rule decides it: the core holds the request (permission.prompt) until we answer with
  // permissions.answer. Shown for the tab on screen; "Remember" stores a permissions.set rule.
  const chrome = useChrome();
  const prompt = $derived(chrome.prompts.find((p) => p.tab === chrome.tabs.activeId) ?? null);
  let remember = $state(true);
</script>

{#if prompt}
  <div class="bar" role="alertdialog" aria-label={t('components.permission.label')} data-prompt={prompt.id}>
    <Icon name="info" size={15} />
    <span class="text"><b>{prompt.origin}</b> {t('components.permission.wants', { what: permissionText(prompt.kinds) })}</span>
    <label class="remember">
      <input type="checkbox" bind:checked={remember} />
      {t('components.permission.remember')}
    </label>
    <button class="allow" onclick={() => chrome.answerPermission(prompt.id, true, remember)}>{t('components.permission.allow')}</button>
    <button onclick={() => chrome.answerPermission(prompt.id, false, remember)}>{t('components.permission.block')}</button>
  </div>
{/if}

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 10px;
    min-height: 38px;
    padding: 4px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--toolbar);
    color: var(--fg);
    font-size: var(--fs-sm);
  }
  .text {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .remember {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--fg-muted);
    white-space: nowrap;
  }
  button {
    height: 26px;
    padding: 0 12px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
    color: var(--fg);
    font-size: var(--fs-sm);
  }
  .allow {
    border-color: transparent;
    background: var(--accent);
    color: white;
  }
</style>
