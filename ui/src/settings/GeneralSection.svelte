<script lang="ts">
  import { LOCALES, systemLocale, t } from '../lib/i18n/index.svelte';
  import { ACCENTS, settings, type Density, type ThemePref, type WidgetId } from '../lib/settings.svelte';
  import Toggle from './Toggle.svelte';

  const THEMES: { id: ThemePref; label: string }[] = [
    { id: 'system', label: 'System' },
    { id: 'light', label: 'Light' },
    { id: 'dark', label: 'Dark' },
  ];
  const DENSITIES: { id: Density; label: string }[] = [
    { id: 'comfortable', label: 'Comfortable' },
    { id: 'compact', label: 'Compact' },
  ];
  const WIDGETS: { id: WidgetId; label: string }[] = [
    { id: 'prompt', label: 'Ask the AI' },
    { id: 'agents', label: 'Agents & runs' },
    { id: 'scripts', label: 'Quick launch scripts' },
    { id: 'sites', label: 'Top sites' },
  ];
  const s = $derived(settings.value);
</script>

<div class="set-page">
  <header>
    <h2>General</h2>
    <p>How the browser looks. Saved on this computer and applied to every window right away.</p>
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
  </section>

  <section class="set-card" aria-labelledby="g-look">
    <h3 id="g-look">Appearance</h3>
    <div class="set-row">
      <div class="text"><span class="label">Theme</span><span class="help">System follows the Windows light / dark setting.</span></div>
      <div class="ctl">
        <div class="set-seg" role="radiogroup" aria-label="Theme">
          {#each THEMES as t (t.id)}
            <button role="radio" aria-checked={s.theme === t.id} onclick={() => settings.update({ theme: t.id })}>{t.label}</button>
          {/each}
        </div>
      </div>
    </div>
    <div class="set-row">
      <div class="text"><span class="label">Accent color</span><span class="help">Buttons, the selected tab and focus rings.</span></div>
      <div class="ctl swatches" role="radiogroup" aria-label="Accent color">
        {#each ACCENTS as c (c)}
          <button
            class="sw"
            role="radio"
            aria-checked={s.accent === c}
            aria-label="Accent {c}"
            style:--c={c}
            onclick={() => settings.update({ accent: c })}
          ></button>
        {/each}
        <label class="custom" title="Custom color">
          <input type="color" value={s.accent} oninput={(e) => settings.update({ accent: e.currentTarget.value })} aria-label="Custom accent color" />
        </label>
      </div>
    </div>
    <div class="set-row">
      <div class="text"><span class="label">Density</span><span class="help">Compact makes the tab strip, toolbar and tabs shorter.</span></div>
      <div class="ctl">
        <div class="set-seg" role="radiogroup" aria-label="Density">
          {#each DENSITIES as d (d.id)}
            <button role="radio" aria-checked={(s.density ?? 'comfortable') === d.id} onclick={() => settings.update({ density: d.id })}>{d.label}</button>
          {/each}
        </div>
      </div>
    </div>
  </section>

  <section class="set-card" aria-labelledby="g-tabs">
    <h3 id="g-tabs">Tabs and toolbar</h3>
    <div class="set-row">
      <div class="text"><span class="label">Vertical tabs</span><span class="help">Tabs in a sidebar on the left instead of a strip on top.</span></div>
      <div class="ctl"><Toggle label="Vertical tabs" checked={s.verticalTabs} onchange={(on) => settings.update({ verticalTabs: on })} /></div>
    </div>
    <div class="set-row">
      <div class="text"><span class="label">Bookmarks bar</span><span class="help">The bar under the toolbar with the bookmarks bar folder.</span></div>
      <div class="ctl"><Toggle label="Bookmarks bar" checked={s.bookmarksBar} onchange={(on) => settings.update({ bookmarksBar: on })} /></div>
    </div>
  </section>

  <section class="set-card" aria-labelledby="g-ntp">
    <h3 id="g-ntp">New tab page</h3>
    {#each WIDGETS as w (w.id)}
      <div class="set-row">
        <div class="text"><span class="label">{w.label}</span></div>
        <div class="ctl">
          <Toggle label={w.label} checked={s.widgets[w.id]} onchange={(on) => settings.update({ widgets: { ...s.widgets, [w.id]: on } })} />
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
