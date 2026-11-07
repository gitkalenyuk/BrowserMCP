<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { ConnectionStatus } from '../lib/api/types';
  import Icon, { type IconName } from '../lib/components/Icon.svelte';
  import { applyLang, locale, t } from '../lib/i18n/index.svelte';
  import { applyTheme, settings } from '../lib/settings.svelte';
  import {
    isSettingsSection,
    onSettingsRequest,
    parseGuideHash,
    peekGuideAnchor,
    takeSettingsRequest,
    type SettingsSection,
  } from '../lib/settings/open';
  import { load, save } from '../lib/storage';
  import AiSection from './AiSection.svelte';
  import GeneralSection from './GeneralSection.svelte';
  import { helpFor, parseAnchor, type GuideAnchor } from './guide/anchors';
  import { GUIDE } from './guide/content';
  import Guide from './guide/Guide.svelte';
  import HintsSection from './HintsSection.svelte';
  import KeyboardSection from './KeyboardSection.svelte';
  import NotificationsSection from './NotificationsSection.svelte';
  import PerformanceSection from './PerformanceSection.svelte';
  import PluginsSection from './PluginsSection.svelte';
  import PrivacySection from './PrivacySection.svelte';
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
  // Voice dictation: engines, local Whisper downloads, hotkey, test.
  import VoiceSection from './VoiceSection.svelte';

  let { api }: { api: BrowserApi } = $props();

  // Labels are dictionary keys: t() must run in markup / $derived so they follow the UI language.
  const SECTIONS: { id: SettingsSection; key: string; icon: IconName }[] = [
    { id: 'general', key: 'settings.nav.general', icon: 'settings' },
    { id: 'startup', key: 'settings.nav.startup', icon: 'home' },
    { id: 'search', key: 'settings.nav.search', icon: 'search' },
    { id: 'downloads', key: 'settings.nav.downloads', icon: 'download' },
    { id: 'sites', key: 'settings.nav.sites', icon: 'globe' },
    { id: 'data', key: 'settings.nav.data', icon: 'folder' },
    { id: 'ai', key: 'settings.nav.ai', icon: 'sparkles' },
    { id: 'voice', key: 'settings.nav.voice', icon: 'mic' },
    { id: 'connections', key: 'settings.nav.connections', icon: 'bot' },
    { id: 'notifications', key: 'settings.nav.notifications', icon: 'alert' },
    { id: 'performance', key: 'settings.nav.performance', icon: 'runs' },
    { id: 'privacy', key: 'settings.nav.privacy', icon: 'lock' },
    { id: 'profiles', key: 'settings.nav.profiles', icon: 'eye' },
    { id: 'proxies', key: 'settings.nav.proxies', icon: 'traffic' },
    { id: 'hints', key: 'settings.nav.hints', icon: 'hand' },
    { id: 'plugins', key: 'settings.nav.plugins', icon: 'layers' },
    { id: 'recording', key: 'settings.nav.recording', icon: 'record' },
    { id: 'keyboard', key: 'settings.nav.keyboard', icon: 'command' },
    { id: 'about', key: 'settings.nav.about', icon: 'info' },
  ];

  // Where the page opens: a `#guide/<chapter>[/<section>]` anchor, else a request from the
  // chrome (localStorage: the URL has no query), else where the user left it.
  const hashAnchor = parseAnchor(parseGuideHash(location.hash));
  const requestAnchor = parseAnchor(peekGuideAnchor());
  const requested = takeSettingsRequest();
  const stored = load<string>('settings.section', 'general');
  let section = $state<SettingsSection>(hashAnchor ? 'guide' : (requested ?? (isSettingsSection(stored) ? stored : 'general')));
  /** Where the guide should go next (a new object per jump; cleared once the guide went there). */
  let guideTarget = $state<{ anchor: GuideAnchor | null } | null>(
    hashAnchor || requestAnchor ? { anchor: hashAnchor ?? requestAnchor } : null,
  );
  let status = $state<ConnectionStatus>('connecting');
  let toast = $state<{ text: string; error: boolean } | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  let main = $state<HTMLElement>();

  clearHash();

  const help = $derived(helpFor(section));
  const helpTitle = $derived(help ? (GUIDE[locale()].find((c) => c.id === help.chapter)?.title ?? '') : '');
  const sectionLabel = $derived(t(SECTIONS.find((s) => s.id === section)?.key ?? `settings.nav.${section}`));

  $effect(() => applyTheme(document.documentElement, settings.value));
  $effect(() => applyLang(document.documentElement));
  $effect(() => api.onStatus((s) => (status = s)));
  $effect(() =>
    onSettingsRequest(() => {
      const anchor = parseAnchor(peekGuideAnchor());
      const req = takeSettingsRequest();
      if (req) section = req;
      // Without an anchor the guide opens where the user left it (as on first load).
      if (req === 'guide' && anchor) guideTarget = { anchor };
    }),
  );
  $effect(() => {
    save('settings.section', section);
    main?.scrollTo({ top: 0 });
  });
  $effect(() => {
    document.title = t('settings.docTitle', { section: sectionLabel });
  });

  /**
   * A tab may load bmcp://ui/settings.html exactly or with a `#guide/<chapter>[/<section>]`
   * anchor (crates/cef scheme.rs `is_settings_page`). The anchor is a one-time jump: it is read
   * once and removed from the address, so a reload or a restored session opens where the user
   * was, not at the old anchor.
   */
  function clearHash() {
    if (location.hash) history.replaceState(history.state, '', location.pathname + location.search);
  }

  function onHashChange() {
    const anchor = parseAnchor(parseGuideHash(location.hash));
    clearHash();
    if (!anchor) return;
    section = 'guide';
    guideTarget = { anchor };
  }

  function openHelp() {
    // Read `help` before switching: it is derived from `section` (and null for the guide).
    const anchor = help;
    if (!anchor) return;
    guideTarget = { anchor };
    section = 'guide';
  }

  function notify(text: string, error = false) {
    toast = { text, error };
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), error ? 6000 : 3000);
  }
  const fail = (err: unknown) => notify(err instanceof Error ? err.message : String(err), true);
</script>

<svelte:window onhashchange={onHashChange} />

<div class="lib">
  <nav aria-label={t('settings.nav.label')}>
    <h1>{t('settings.title')}</h1>
    {#each SECTIONS as s (s.id)}
      <button class="nav-item" aria-current={section === s.id ? 'page' : undefined} onclick={() => (section = s.id)}>
        <Icon name={s.icon} size={16} />
        <span>{t(s.key)}</span>
      </button>
    {/each}
    <hr />
    <button class="nav-item" aria-current={section === 'guide' ? 'page' : undefined} onclick={() => (section = 'guide')}>
      <Icon name="info" size={16} />
      <span>{t('settings.nav.guide')}</span>
    </button>
    {#if api.kind === 'ws' && status !== 'open'}
      <p class="conn" role="status">{status === 'connecting' ? t('settings.conn.connecting') : t('settings.conn.closed')}</p>
    {:else if api.kind === 'mock'}
      <p class="conn mock">{t('settings.conn.mock')}</p>
    {/if}
  </nav>

  <main bind:this={main}>
    <!-- Shared header area of every section: the link to its chapter of the guide. -->
    <div class="section-wrap">
      {#if help}
        <button class="help-link" onclick={openHelp} title={t('settings.helpTitle', { chapter: helpTitle })}>
          <span class="q" aria-hidden="true">?</span>
          <span>{t('settings.help')}</span>
          <span class="sr-only">: {helpTitle}</span>
        </button>
      {/if}
      {#key section}
        {#if section === 'general'}
          <GeneralSection {api} {notify} />
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
        {:else if section === 'connections'}
          <ConnectionsSection {api} {notify} {fail} />
        {:else if section === 'profiles'}
          <ProfilesSection {api} {notify} {fail} />
        {:else if section === 'proxies'}
          <ProxiesSection {api} {notify} {fail} />
        {:else if section === 'recording'}
          <RecordingSection {api} {notify} {fail} />
        {:else if section === 'ai'}
          <AiSection {api} />
        {:else if section === 'voice'}
          <VoiceSection {api} {notify} {fail} />
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
        {:else if section === 'guide'}
          <Guide
            target={guideTarget}
            onconsumed={() => (guideTarget = null)}
            onopen={(id) => (section = id)}
            onfail={(m) => notify(m, true)}
          />
        {:else}
          <KeyboardSection {notify} />
        {/if}
      {/key}
    </div>
  </main>

  {#if toast}
    <div class="toast" class:error={toast.error} role={toast.error ? 'alert' : 'status'}>
      <span>{toast.text}</span>
      <button class="icon-btn" aria-label={t('settings.toast.dismiss')} onclick={() => (toast = null)}><Icon name="x" size={14} /></button>
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
  hr {
    width: calc(100% - 20px);
    margin: 8px 10px;
    border: 0;
    border-top: 1px solid var(--border);
  }
  /* The sections are 940 px wide pages; the Help link sits in their top right corner. */
  .section-wrap {
    position: relative;
  }
  .help-link {
    position: absolute;
    z-index: 2;
    top: 26px;
    left: min(calc(100% - 32px), 908px);
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 10px 0 4px;
    border: 1px solid var(--border);
    border-radius: 13px;
    background: var(--surface);
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    font-weight: 550;
    white-space: nowrap;
    transform: translateX(-100%);
  }
  .help-link:hover {
    border-color: var(--accent-line);
    color: var(--accent);
  }
  .help-link .q {
    display: inline-grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: var(--fs-xs);
    font-weight: 700;
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
    hr {
      display: none;
    }
    /* No room beside the section title: the link goes above it. */
    .help-link {
      position: static;
      margin: 12px 16px -6px auto;
      transform: none;
    }
    .section-wrap {
      display: flex;
      flex-direction: column;
    }
  }
</style>
