<script lang="ts">
  import { settings, type PanelId } from '../settings.svelte';
  import Icon, { type IconName } from './Icon.svelte';
  import ResizeHandle from './ResizeHandle.svelte';
  import AiPanel from './panels/AiPanel.svelte';
  import EmptyPanel from './panels/EmptyPanel.svelte';
  import ScriptsPanel from './panels/ScriptsPanel.svelte';
  import TerminalPanel from './panels/TerminalPanel.svelte';
  import TrafficPanel from './panels/TrafficPanel.svelte';

  let { panel }: { panel: PanelId } = $props();

  const META: Record<PanelId, { title: string; icon: IconName }> = {
    traffic: { title: 'Traffic', icon: 'traffic' },
    recording: { title: 'Recording', icon: 'record' },
    ai: { title: 'AI', icon: 'sparkles' },
    terminal: { title: 'Terminal', icon: 'terminal' },
    scripts: { title: 'Scripts', icon: 'code' },
    runs: { title: 'Runs', icon: 'runs' },
  };
  const meta = $derived(META[panel]);
</script>

<aside class="panel" style:width="{settings.value.panelWidth}px" aria-label="{meta.title} panel">
  <ResizeHandle
    edge="left"
    value={settings.value.panelWidth}
    min={280}
    max={720}
    onchange={(w) => settings.update({ panelWidth: w })}
  />
  <header>
    <Icon name={meta.icon} size={15} />
    <h2>{meta.title}</h2>
    <button class="icon-btn close" aria-label="Close panel" title="Close panel" onclick={() => settings.update({ panel: null })}>
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
      <EmptyPanel
        icon="record"
        title="Not recording"
        text="Record clicks, typing and traffic in this tab, optionally with video. A recording turns into a TypeScript script you can replay with different data."
        action="Start recording"
      />
    {:else if panel === 'scripts'}
      <ScriptsPanel />
    {:else if panel === 'runs'}
      <EmptyPanel
        icon="runs"
        title="No runs yet"
        text="Run a script over a CSV or JSON file across several profiles. Progress, failures and patches waiting for your approval show up here."
        action="New run"
      />
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
