<script lang="ts">
  import { t } from '../i18n/index.svelte';
  import { settings, type PanelId } from '../settings.svelte';
  import Icon, { type IconName } from './Icon.svelte';
  import ResizeHandle from './ResizeHandle.svelte';
  import AiPanel from './panels/AiPanel.svelte';
  import RecordingPanel from './panels/RecordingPanel.svelte';
  import RunsPanel from './panels/RunsPanel.svelte';
  import ScriptsPanel from './panels/ScriptsPanel.svelte';
  import TerminalPanel from './panels/TerminalPanel.svelte';
  import TrafficPanel from './panels/TrafficPanel.svelte';

  let { panel }: { panel: PanelId } = $props();

  const ICONS: Record<PanelId, IconName> = {
    traffic: 'traffic',
    recording: 'record',
    ai: 'sparkles',
    terminal: 'terminal',
    scripts: 'code',
    runs: 'runs',
  };
  const title = $derived(t(`components.panel.${panel}`));
</script>

<aside class="panel" style:width="{settings.value.panelWidth}px" aria-label={t('components.panelLabel', { name: title })}>
  <ResizeHandle
    edge="left"
    value={settings.value.panelWidth}
    min={280}
    max={720}
    onchange={(w) => settings.update({ panelWidth: w })}
  />
  <header>
    <Icon name={ICONS[panel]} size={15} />
    <h2>{title}</h2>
    <button class="icon-btn close" aria-label={t('components.closePanel')} title={t('components.closePanel')} onclick={() => settings.update({ panel: null })}>
      <Icon name="x" size={15} />
    </button>
  </header>
  <div class="body">
    {#if panel === 'ai'}
      <AiPanel />
    {:else if panel === 'terminal'}
      <TerminalPanel />
    {:else if panel === 'traffic'}
      <TrafficPanel />
    {:else if panel === 'recording'}
      <RecordingPanel />
    {:else if panel === 'scripts'}
      <ScriptsPanel />
    {:else if panel === 'runs'}
      <RunsPanel />
    {/if}
  </div>
</aside>

<style>
  .panel {
    position: relative;
    display: flex;
    flex-direction: column;
    flex: none;
    min-height: 0;
    background: var(--toolbar);
    border-left: 1px solid var(--border);
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 38px;
    padding: 0 4px 0 12px;
    color: var(--fg-muted);
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  h2 {
    flex: 1;
    margin: 0;
    color: var(--fg);
    font-size: var(--fs);
    font-weight: 650;
  }
  .close {
    width: 28px;
    height: 28px;
  }
  .body {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }
</style>
