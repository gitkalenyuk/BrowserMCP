<script lang="ts">
  import { createApi, type BrowserApi } from '../lib/api';
  import { LOCALES, systemLocale, t } from '../lib/i18n/index.svelte';
  import { ACCENTS, settings, type Density, type ThemePref, type WidgetId } from '../lib/settings.svelte';
  import Toggle from './Toggle.svelte';
  import WebLanguages from './WebLanguages.svelte';

  // `api` / `notify` come from the settings page; without them (an older Settings.svelte) the
  // language card opens its own connection and shows its errors inline.
  let { api, notify }: { api?: BrowserApi; notify?: (text: string, error?: boolean) => void } = $props();
  const languagesApi = $derived(api ?? createApi());

  // Labels are looked up in markup (t() follows the interface language).
  const THEMES: ThemePref[] = ['system', 'light', 'dark'];
  const DENSITIES: Density[] = ['comfortable', 'compact'];
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

  <section class="set-card" aria-labelledby="g-look">
    <h3 id="g-look">{t('settings.general.appearance')}</h3>
    <div class="set-row">
      <div class="text"><span class="label">{t('settings.general.theme')}</span><span class="help">{t('settings.general.themeHelp')}</span></div>
      <div class="ctl">
        <div class="set-seg" role="radiogroup" aria-label={t('settings.general.theme')}>
          {#each THEMES as theme (theme)}
            <button role="radio" aria-checked={s.theme === theme} onclick={() => settings.update({ theme })}>{t(`settings.general.theme.${theme}`)}</button>
          {/each}
        </div>
      </div>
    </div>
    <div class="set-row">
      <div class="text"><span class="label">{t('settings.general.accent')}</span><span class="help">{t('settings.general.accentHelp')}</span></div>
      <div class="ctl swatches" role="radiogroup" aria-label={t('settings.general.accent')}>
        {#each ACCENTS as c (c)}
          <button
            class="sw"
            role="radio"
            aria-checked={s.accent === c}
            aria-label={t('settings.general.accentN', { color: c })}
            style:--c={c}
            onclick={() => settings.update({ accent: c })}
          ></button>
        {/each}
        <label class="custom" title={t('settings.general.customColor')}>
          <input type="color" value={s.accent} oninput={(e) => settings.update({ accent: e.currentTarget.value })} aria-label={t('settings.general.customAccent')} />
        </label>
      </div>
    </div>
    <div class="set-row">
      <div class="text"><span class="label">{t('settings.general.density')}</span><span class="help">{t('settings.general.densityHelp')}</span></div>
      <div class="ctl">
        <div class="set-seg" role="radiogroup" aria-label={t('settings.general.density')}>
          {#each DENSITIES as density (density)}
            <button role="radio" aria-checked={(s.density ?? 'comfortable') === density} onclick={() => settings.update({ density })}>{t(`settings.general.density.${density}`)}</button>
          {/each}
        </div>
      </div>
    </div>
  </section>

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

<style>
  .swatches {
    gap: 6px;
  }
  .sw {
    width: 22px;
    height: 22px;
    padding: 0;
    border: 2px solid transparent;
    border-radius: 50%;
    background: var(--c);
    box-shadow: inset 0 0 0 2px var(--surface);
  }
  .sw[aria-checked='true'] {
    border-color: var(--c);
  }
  .custom {
    display: grid;
    width: 26px;
    height: 26px;
    overflow: hidden;
    border: 1px dashed var(--border-strong);
    border-radius: 50%;
  }
  .custom input {
    width: 40px;
    height: 40px;
    margin: -8px;
    border: 0;
    padding: 0;
    background: none;
  }
</style>
