<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { ConnectionStatus } from '../lib/api/types';
  import Icon, { type IconName } from '../lib/components/Icon.svelte';
  import { applyTheme, settings } from '../lib/settings.svelte';
  import { isSettingsSection, onSettingsRequest, takeSettingsRequest, type SettingsSection } from '../lib/settings/open';
  import { load, save } from '../lib/storage';
  import AiSection from './AiSection.svelte';
  import GeneralSection from './GeneralSection.svelte';
  import HintsSection from './HintsSection.svelte';
  import KeyboardSection from './KeyboardSection.svelte';
  import NotificationsSection from './NotificationsSection.svelte';
  import PerformanceSection from './PerformanceSection.svelte';
  import PluginsSection from './PluginsSection.svelte';
  import PrivacySection from './PrivacySection.svelte';
  import { t } from '../lib/i18n/index.svelte';
  import AboutSection from './AboutSection.svelte';
  import DataSection from './DataSection.svelte';
  import DownloadsSection from './DownloadsSection.svelte';
  import SearchSection from './SearchSection.svelte';
  import SiteSettingsSection from './SiteSettingsSection.svelte';
  import StartupSection from './StartupSection.svelte';
  // Stream E2: Connections & API, Profiles & identity, Proxies, Recording & scripts.
  import ConnectionsSection from './ConnectionsSection.svelte';
  import ProfilesSection from './ProfilesSection.svelte';
  import ProxiesSection from './ProxiesSection.svelte';
  import RecordingSection from './RecordingSection.svelte';

  let { api }: { api: BrowserApi } = $props();

  const SECTIONS: { id: SettingsSection; label: string; icon: IconName }[] = [
    { id: 'general', label: 'General', icon: 'settings' },
    // Getters: t() is read when the nav renders, so the labels follow the UI language.
    { id: 'startup', get label() { return t('settings.startup.nav'); }, icon: 'home' },
    { id: 'search', get label() { return t('settings.search.nav'); }, icon: 'search' },
    { id: 'downloads', get label() { return t('settings.downloads.nav'); }, icon: 'download' },
    { id: 'sites', get label() { return t('settings.sites.nav'); }, icon: 'globe' },
    { id: 'data', get label() { return t('settings.data.nav'); }, icon: 'folder' },
    { id: 'ai', label: 'AI', icon: 'sparkles' },
    // Getters: t() is read while rendering, so the label follows the UI language.
    { id: 'connections', get label() { return t('settings.connections.nav'); }, icon: 'bot' },
    { id: 'notifications', label: 'Notifications', icon: 'alert' },
    { id: 'performance', label: 'Performance', icon: 'runs' },
    { id: 'privacy', label: 'Privacy & blocking', icon: 'lock' },
    { id: 'profiles', get label() { return t('settings.profiles.nav'); }, icon: 'eye' },
    { id: 'proxies', get label() { return t('settings.proxies.nav'); }, icon: 'traffic' },
    { id: 'hints', label: 'Link hints', icon: 'hand' },
    { id: 'plugins', label: 'Plugins & userscripts', icon: 'layers' },
    { id: 'recording', get label() { return t('settings.recording.nav'); }, icon: 'record' },
    { id: 'keyboard', label: 'Keyboard', icon: 'command' },
    { id: 'about', get label() { return t('settings.about.nav'); }, icon: 'info' },
  ];

  // The chrome asks for a section through localStorage (the URL has no query); otherwise the
  // page opens where the user left it.
  const stored = load<string>('settings.section', 'general');
  let section = $state<SettingsSection>(takeSettingsRequest() ?? (isSettingsSection(stored) ? stored : 'general'));
  let status = $state<ConnectionStatus>('connecting');
  let toast = $state<{ text: string; error: boolean } | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let main = $state<HTMLElement>();

  $effect(() => applyTheme(document.documentElement, settings.value));
  $effect(() => api.onStatus((s) => (status = s)));
  $effect(() =>
    onSettingsRequest(() => {
      const req = takeSettingsRequest();
      if (req) section = req;
    }),
  );
  $effect(() => {
    save('settings.section', section);
    main?.scrollTo({ top: 0 });
  });
  $effect(() => {
    const label = SECTIONS.find((s) => s.id === section)?.label ?? 'Settings';
    document.title = `${label} — Settings`;
  });

  function notify(text: string, error = false) {
    toast = { text, error };
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), error ? 6000 : 3000);
  }
  const fail = (err: unknown) => notify(err instanceof Error ? err.message : String(err), true);
</script>

<div class="lib">
  <nav aria-label="Settings">
    <h1>Settings</h1>
    {#each SECTIONS as s (s.id)}
      <button class="nav-item" aria-current={section === s.id ? 'page' : undefined} onclick={() => (section = s.id)}>
        <Icon name={s.icon} size={16} />
        <span>{s.label}</span>
      </button>
    {/each}
    {#if api.kind === 'ws' && status !== 'open'}
      <p class="conn" role="status">{status === 'connecting' ? 'Connecting to the browser…' : 'Disconnected from the browser'}</p>
    {:else if api.kind === 'mock'}
      <p class="conn mock">Demo data (no browser connected)</p>
    {/if}
  </nav>

  <main bind:this={main}>
    {#key section}
      {#if section === 'general'}
        <GeneralSection />
      {:else if section === 'startup'}
        <StartupSection {api} {notify} {fail} />
      {:else if section === 'search'}
        <SearchSection {api} {notify} {fail} />
      {:else if section === 'downloads'}
        <DownloadsSection {api} {notify} {fail} />
      {:else if section === 'sites'}
        <SiteSettingsSection {api} {notify} {fail} />
      {:else if section === 'data'}
        <DataSection {api} {notify} {fail} />
      {:else if section === 'about'}
        <AboutSection {api} {notify} {fail} />
      {:else if section === 'ai'}
        <AiSection {api} />
      {:else if section === 'notifications'}
        <NotificationsSection {api} {notify} {fail} />
      {:else if section === 'performance'}
        <PerformanceSection {api} {notify} {fail} />
      {:else if section === 'privacy'}
        <PrivacySection {api} {notify} {fail} />
      {:else if section === 'hints'}
        <HintsSection {api} {notify} {fail} />
      {:else if section === 'plugins'}
        <PluginsSection {api} {notify} {fail} />
      {:else if section === 'connections'}
        <ConnectionsSection {api} {notify} {fail} />
      {:else if section === 'profiles'}
        <ProfilesSection {api} {notify} {fail} />
      {:else if section === 'proxies'}
        <ProxiesSection {api} {notify} {fail} />
      {:else if section === 'recording'}
        <RecordingSection {api} {notify} {fail} />
      {:else}
        <KeyboardSection {notify} />
      {/if}
    {/key}
  </main>

  {#if toast}
    <div class="toast" class:error={toast.error} role={toast.error ? 'alert' : 'status'}>
      <span>{toast.text}</span>
      <button class="icon-btn" aria-label="Dismiss" onclick={() => (toast = null)}><Icon name="x" size={14} /></button>
    </div>
  {/if}
</div>

<style>
  .lib {
    display: grid;
    grid-template-columns: 232px minmax(0, 1fr);
    height: 100vh;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 18px 10px;
    border-right: 1px solid var(--border);
    background: var(--toolbar);
    overflow-y: auto;
  }
  h1 {
    margin: 0 10px 14px;
    font-size: 18px;
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 34px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--r);
    background: transparent;
    color: var(--fg-muted);
    font-weight: 550;
    text-align: left;
  }
  .nav-item:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .nav-item[aria-current='page'] {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .conn {
    margin: auto 10px 0;
    padding-top: 12px;
    color: var(--warn);
    font-size: var(--fs-sm);
  }
  .conn.mock {
    color: var(--fg-subtle);
  }
  main {
    min-width: 0;
    overflow: auto;
    background: var(--toolbar);
  }
  .toast {
    position: fixed;
    z-index: 50;
    left: 50%;
    bottom: 18px;
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: min(560px, calc(100vw - 32px));
    padding: 6px 6px 6px 14px;
    border-radius: var(--r-lg);
    background: var(--fg);
    color: var(--toolbar);
    font-size: var(--fs-sm);
    box-shadow: var(--shadow-pop);
    transform: translateX(-50%);
  }
  .toast.error {
    background: var(--danger);
    color: #fff;
  }
  .toast .icon-btn {
    color: inherit;
  }
  @media (max-width: 720px) {
    .lib {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: auto minmax(0, 1fr);
    }
    nav {
      flex-direction: row;
      flex-wrap: wrap;
      padding: 10px;
      border-right: 0;
      border-bottom: 1px solid var(--border);
    }
    h1,
    .conn {
      display: none;
    }
    .nav-item span {
      display: none;
    }
  }
</style>
