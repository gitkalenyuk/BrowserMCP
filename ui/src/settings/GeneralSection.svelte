<script lang="ts" module>
  import { createApi, type BrowserApi } from '../lib/api';

  // Fallback while Settings.svelte renders <GeneralSection /> without `api`: one connection per
  // page, shared by every visit to this section (a new one per mount would leak a WebSocket and,
  // in mock mode, reset the card's state each time the section is opened).
  let ownApi: BrowserApi | undefined;
  const sharedApi = () => (ownApi ??= createApi());
</script>

<script lang="ts">
  import { LOCALES, systemLocale, t } from '../lib/i18n/index.svelte';
  import { settings, type WidgetId } from '../lib/settings.svelte';
  import AppearanceCard from './AppearanceCard.svelte';
  import Toggle from './Toggle.svelte';
  import WebLanguages from './WebLanguages.svelte';

  // `api` / `notify` come from the settings page; without them (an older Settings.svelte) the
  // language card uses the page-wide fallback connection and shows its messages inline.
  let { api, notify }: { api?: BrowserApi; notify?: (text: string, error?: boolean) => void } = $props();
  const languagesApi = $derived(api ?? sharedApi());

  // Labels are looked up in markup (t() follows the interface language).
  const WIDGETS: WidgetId[] = ['prompt', 'agents', 'scripts', 'sites'];
  const s = $derived(settings.value);
</script>

<div class="set-page">
  <header>
    <h2>{t('settings.general.title')}</h2>
    <p>{t('settings.general.intro')}</p>
  </header>

  <section class="set-card" aria-labelledby="g-lang">
    <h3 id="g-lang">{t('settings.language.label')}</h3>
    <div class="set-row">
      <div class="text"><span class="label">{t('settings.language.ui')}</span><span class="help">{t('settings.language.uiHint')}</span></div>
      <div class="ctl">
        <div class="set-seg" role="radiogroup" aria-label={t('settings.language.ui')}>
          <button role="radio" aria-checked={s.language === 'system'} onclick={() => settings.update({ language: 'system' })}>
            {t('settings.language.system', { name: LOCALES.find((l) => l.id === systemLocale())?.name ?? 'English' })}
          </button>
          {#each LOCALES as l (l.id)}
            <button role="radio" lang={l.id} aria-checked={s.language === l.id} onclick={() => settings.update({ language: l.id })}>{l.name}</button>
          {/each}
        </div>
      </div>
    </div>
    <WebLanguages api={languagesApi} {notify} />
  </section>

  <AppearanceCard />

  <section class="set-card" aria-labelledby="g-tabs">
    <h3 id="g-tabs">{t('settings.general.tabs')}</h3>
    <div class="set-row">
      <div class="text"><span class="label">{t('settings.general.vertical')}</span><span class="help">{t('settings.general.verticalHelp')}</span></div>
      <div class="ctl"><Toggle label={t('settings.general.vertical')} checked={s.verticalTabs} onchange={(on) => settings.update({ verticalTabs: on })} /></div>
    </div>
    <div class="set-row">
      <div class="text"><span class="label">{t('settings.general.bookmarksBar')}</span><span class="help">{t('settings.general.bookmarksBarHelp')}</span></div>
      <div class="ctl"><Toggle label={t('settings.general.bookmarksBar')} checked={s.bookmarksBar} onchange={(on) => settings.update({ bookmarksBar: on })} /></div>
    </div>
  </section>

  <section class="set-card" aria-labelledby="g-ntp">
    <h3 id="g-ntp">{t('settings.general.newTab')}</h3>
    {#each WIDGETS as w (w)}
      <div class="set-row">
        <div class="text"><span class="label">{t(`settings.general.widget.${w}`)}</span></div>
        <div class="ctl">
          <Toggle label={t(`settings.general.widget.${w}`)} checked={s.widgets[w]} onchange={(on) => settings.update({ widgets: { ...s.widgets, [w]: on } })} />
        </div>
      </div>
    {/each}
  </section>
</div>
