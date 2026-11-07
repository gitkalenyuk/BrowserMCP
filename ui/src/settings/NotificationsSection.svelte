<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { NotifyChannel, NotifyConfig } from '../lib/api/types-settings';
  import Icon from '../lib/components/Icon.svelte';
  import { t, tp } from '../lib/i18n/index.svelte';
  import {
    CHANNELS,
    KNOWN_TOPICS,
    WHEN,
    addRule,
    configErrors,
    configPatch,
    firstMatch,
    isEmptyPatch,
    isMaskedToken,
    missingDefaults,
    moveRule,
    removeRule,
    toggleChannel,
    updateRule,
  } from '../lib/settings/notify';
  import { richParts } from '../lib/settings/rich';
  import Toggle from './Toggle.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (t: string, e?: boolean) => void; fail: (e: unknown) => void } = $props();

  let saved = $state<NotifyConfig | null>(null);
  let draft = $state<NotifyConfig | null>(null);
  let loadError = $state<string | null>(null);
  let saving = $state(false);
  let testing = $state<NotifyChannel | null>(null);
  let results = $state<Partial<Record<NotifyChannel, { ok: boolean; text: string }>>>({});
  let probe = $state('run.heal_proposed');
  let showToken = $state(false);

  async function load() {
    try {
      const c = await api.call('notify.config_get');
      saved = c;
      draft = structuredClone(c);
      loadError = null;
    } catch (err) {
      loadError = (err as Error).message;
    }
  }
  $effect(() => {
    void load();
  });

  const errors = $derived(draft ? configErrors(draft) : {});
  const patch = $derived(saved && draft ? configPatch(saved, draft) : {});
  const dirty = $derived(!isEmptyPatch(patch));
  const invalid = $derived(Object.keys(errors).length > 0);
  const missing = $derived(draft ? missingDefaults(draft.rules) : []);
  const probeHit = $derived(draft && probe.trim() ? firstMatch(draft.rules, probe) : -1);

  async function save(): Promise<boolean> {
    if (!saved || !draft || invalid) return false;
    if (!dirty) return true;
    saving = true;
    try {
      const c = await api.call('notify.config_set', patch);
      saved = c;
      draft = structuredClone(c);
      notify(t('settings.notify.saved'));
      return true;
    } catch (err) {
      fail(err);
      return false;
    } finally {
      saving = false;
    }
  }

  async function test(channel: NotifyChannel) {
    if (dirty && !(await save())) return;
    testing = channel;
    results = { ...results, [channel]: undefined };
    try {
      const r = await api.call('notify.test', { channel });
      results = { ...results, [channel]: { ok: r.ok, text: r.ok ? t('settings.notify.testSent') : (r.error ?? t('settings.notify.testFailed')) } };
    } catch (err) {
      results = { ...results, [channel]: { ok: false, text: (err as Error).message } };
    } finally {
      testing = null;
    }
  }

  const tg = $derived(draft?.channels.telegram);
  const wh = $derived(draft?.channels.webhook);
</script>

{#snippet testButton(channel: NotifyChannel)}
  <button class="btn" disabled={testing !== null || saving || (dirty && invalid)} onclick={() => test(channel)} title={dirty ? t('settings.notify.saveTestHint') : t('settings.notify.testHint')}>
    {testing === channel ? t('settings.notify.sending') : dirty ? t('settings.notify.saveTest') : t('common.test')}
  </button>
{/snippet}
{#snippet result(channel: NotifyChannel)}
  {#if results[channel]}
    <p class={results[channel].ok ? 'set-ok' : 'set-err'} role="status">{results[channel].text}</p>
  {/if}
{/snippet}

<div class="set-page">
  <header>
    <h2>{t('settings.notify.title')}</h2>
    <p>
      {t('settings.notify.intro')}
      <span class="set-mono">data/config/notify.json</span>.
    </p>
  </header>

  {#if loadError}
    <p class="lib-error" role="alert">{t('settings.notify.loadError', { error: loadError })}</p>
    <button class="btn" onclick={load}>{t('settings.hints.retry')}</button>
  {:else if !draft || !tg || !wh}
    <p class="set-muted">{t('common.loading')}</p>
  {:else}
    <section class="set-card" aria-labelledby="n-ch">
      <h3 id="n-ch">{t('settings.notify.channels')}</h3>
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.notify.channel.ui')}</span>
          <span class="help">{t('settings.notify.uiHelp')}</span>
          {@render result('ui')}
        </div>
        <div class="ctl">
          {@render testButton('ui')}
          <Toggle label={t('settings.notify.uiToggle')} checked={draft.channels.ui} onchange={(on) => (draft!.channels.ui = on)} />
        </div>
      </div>
      <div class="set-row">
        <div class="text">
          <span class="label">{t('settings.notify.channel.os')}</span>
          <span class="help">{t('settings.notify.osHelp')}</span>
          {@render result('os')}
        </div>
        <div class="ctl">
          {@render testButton('os')}
          <Toggle label={t('settings.notify.osToggle')} checked={draft.channels.os} onchange={(on) => (draft!.channels.os = on)} />
        </div>
      </div>

      <div class="set-row stack">
        <div class="head">
          <div class="text">
            <span class="label">{t('settings.notify.channel.telegram')}</span>
            <span class="help">
              {t('settings.notify.tgHelp1')}
              <span class="set-mono">@BotFather</span> (<span class="set-mono">/newbot</span>){t('settings.notify.tgHelp2')}
              <span class="set-mono">@userinfobot</span>{t('settings.notify.tgHelp3')}
            </span>
          </div>
          <div class="ctl">
            {@render testButton('telegram')}
            <Toggle label={t('settings.notify.tgToggle')} checked={tg.enabled} onchange={(on) => (tg.enabled = on)} />
          </div>
        </div>
        <div class="grid">
          <label class="lib-field">
            {t('settings.notify.botToken')}
            <span class="pw">
              <input
                class="lib-input mono"
                type={showToken ? 'text' : 'password'}
                autocomplete="off"
                spellcheck="false"
                bind:value={tg.bot_token}
                placeholder="123456789:AA…"
                aria-invalid={!!errors['telegram.bot_token']}
                onfocus={(e) => isMaskedToken(e.currentTarget.value) && e.currentTarget.select()}
              />
              <button class="icon-btn" type="button" aria-label={showToken ? t('settings.notify.hideToken') : t('settings.notify.showToken')} onclick={() => (showToken = !showToken)}>
                <Icon name={showToken ? 'eye-off' : 'eye'} size={14} />
              </button>
            </span>
            {#if errors['telegram.bot_token']}<span class="set-err">{errors['telegram.bot_token']}</span>
            {:else if isMaskedToken(tg.bot_token)}<span class="set-muted">{t('settings.notify.tokenStored')}</span>{/if}
          </label>
          <label class="lib-field">
            {t('settings.notify.chatId')}
            <input class="lib-input mono" spellcheck="false" bind:value={tg.chat_id} placeholder="123456789" aria-invalid={!!errors['telegram.chat_id']} />
            {#if errors['telegram.chat_id']}<span class="set-err">{errors['telegram.chat_id']}</span>{/if}
          </label>
        </div>
        {@render result('telegram')}
      </div>

      <div class="set-row stack">
        <div class="head">
          <div class="text">
            <span class="label">{t('settings.notify.channel.webhook')}</span>
            <span class="help">
              {#each richParts(t('settings.notify.whHelp')) as p, i (i)}{#if p.slot === 'method'}<span class="set-mono">POST</span>{:else if p.slot === 'body'}<span class="set-mono">{'{id, title, body, level, topic, data, actions, ts_ms}'}</span>{:else}{p.text}{/if}{/each}
            </span>
          </div>
          <div class="ctl">
            {@render testButton('webhook')}
            <Toggle label={t('settings.notify.whToggle')} checked={wh.enabled} onchange={(on) => (wh.enabled = on)} />
          </div>
        </div>
        <label class="lib-field">
          URL
          <input class="lib-input mono" type="url" spellcheck="false" bind:value={wh.url} placeholder="https://hooks.example.com/browsermcp" aria-invalid={!!errors['webhook.url']} />
          {#if errors['webhook.url']}<span class="set-err">{errors['webhook.url']}</span>{/if}
        </label>
        {@render result('webhook')}
      </div>
    </section>

    <section class="set-card" aria-labelledby="n-rules">
      <h3 id="n-rules"><span class="grow">{t('settings.notify.rules')}</span></h3>
      <p class="lead">
        {t('settings.notify.rulesLead')} <b>{t('settings.notify.bundle')}</b> {t('settings.notify.bundleLead')}
      </p>
      <div class="set-table-wrap">
        <table class="set-table rules">
          <thead>
            <tr>
              <th class="r">#</th>
              <th>{t('settings.notify.topic')}</th>
              {#each CHANNELS as c (c.id)}<th class="c">{c.label}</th>{/each}
              <th>{t('settings.notify.when')}</th>
              <th class="c" title={t('settings.notify.bundleHint')}>{t('settings.notify.bundle')}</th>
              <th><span class="sr">{t('settings.notify.orderDelete')}</span></th>
            </tr>
          </thead>
          <tbody>
            {#each draft.rules as rule, i (i)}
              <tr class:bad={!!errors[`rule.${i}`]} class:hit={probeHit === i}>
                <td class="r set-muted">{i + 1}</td>
                <td>
                  <input
                    class="lib-input mono topic"
                    value={rule.topic}
                    list="notify-topics"
                    spellcheck="false"
                    aria-label={t('settings.notify.topicOf', { n: i + 1 })}
                    aria-invalid={!!errors[`rule.${i}`]}
                    oninput={(e) => (draft!.rules = updateRule(draft!.rules, i, { topic: e.currentTarget.value }))}
                  />
                </td>
                {#each CHANNELS as c (c.id)}
                  <td class="c">
                    <input
                      type="checkbox"
                      checked={rule.channels.includes(c.id)}
                      aria-label={t('settings.notify.channelFor', { channel: c.label, rule: rule.topic || t('settings.notify.ruleN', { n: i + 1 }) })}
                      onchange={() => (draft!.rules = toggleChannel(draft!.rules, i, c.id))}
                    />
                  </td>
                {/each}
                <td>
                  <select class="lib-input when" value={rule.when} aria-label={t('settings.notify.conditionOf', { n: i + 1 })} onchange={(e) => (draft!.rules = updateRule(draft!.rules, i, { when: e.currentTarget.value as typeof rule.when }))}>
                    {#each WHEN as w (w.id)}<option value={w.id}>{w.label}</option>{/each}
                  </select>
                </td>
                <td class="c">
                  <input type="checkbox" checked={rule.aggregate} aria-label={t('settings.notify.bundleOf', { n: i + 1 })} onchange={(e) => (draft!.rules = updateRule(draft!.rules, i, { aggregate: e.currentTarget.checked }))} />
                </td>
                <td class="acts">
                  <button class="icon-btn" aria-label={t('settings.notify.moveUp', { n: i + 1 })} disabled={i === 0} onclick={() => (draft!.rules = moveRule(draft!.rules, i, -1))}><Icon name="chevron-down" size={13} class="up" /></button>
                  <button class="icon-btn" aria-label={t('settings.notify.moveDown', { n: i + 1 })} disabled={i === draft.rules.length - 1} onclick={() => (draft!.rules = moveRule(draft!.rules, i, 1))}><Icon name="chevron-down" size={13} /></button>
                  <button class="icon-btn" aria-label={t('settings.notify.deleteRule', { n: i + 1 })} onclick={() => (draft!.rules = removeRule(draft!.rules, i))}><Icon name="trash" size={13} /></button>
                </td>
              </tr>
              {#if errors[`rule.${i}`]}
                <tr class="errrow"><td></td><td colspan={CHANNELS.length + 4}><span class="set-err">{errors[`rule.${i}`]}</span></td></tr>
              {/if}
            {:else}
              <tr><td colspan={CHANNELS.length + 5} class="set-muted">{t('settings.notify.noRules')}</td></tr>
            {/each}
          </tbody>
        </table>
        <datalist id="notify-topics">{#each KNOWN_TOPICS as topic (topic)}<option value={topic}></option>{/each}</datalist>
      </div>
      <div class="set-actions">
        <button class="btn" onclick={() => (draft!.rules = addRule(draft!.rules))}><Icon name="plus" size={13} /> {t('settings.notify.addRule')}</button>
        {#if missing.length}
          <button class="btn" onclick={() => (draft!.rules = [...draft!.rules, ...missing])} title={missing.map((r) => r.topic).join(', ')}>
            {tp('settings.notify.addMissing', missing.length)}
          </button>
        {/if}
        <span class="probe">
          <label for="probe" class="set-muted">{t('settings.notify.probe')}</label>
          <input id="probe" class="lib-input mono" bind:value={probe} list="notify-topics" spellcheck="false" />
          <span class="set-muted" role="status">{probe.trim() ? (probeHit >= 0 ? t('settings.notify.probeHit', { n: probeHit + 1 }) : t('settings.notify.probeNone')) : ''}</span>
        </span>
      </div>
    </section>

    <section class="set-card" aria-labelledby="n-lim">
      <h3 id="n-lim">{t('settings.notify.antispam')}</h3>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.notify.dedup')}</span><span class="help">{t('settings.notify.dedupHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input narrow" type="number" min="0" max="3600" bind:value={draft.limits.dedup_secs} aria-label={t('settings.notify.dedupLabel')} aria-invalid={!!errors['limits.dedup_secs']} /> {t('settings.notify.secs')}
        </div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.notify.window')}</span><span class="help">{t('settings.notify.windowHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input narrow" type="number" min="1" max="3600" bind:value={draft.limits.aggregate_secs} aria-label={t('settings.notify.windowLabel')} aria-invalid={!!errors['limits.aggregate_secs']} /> {t('settings.notify.secs')}
        </div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">{t('settings.notify.perMinute')}</span><span class="help">{t('settings.notify.perMinuteHelp')}</span></div>
        <div class="ctl">
          <input class="lib-input narrow" type="number" min="1" max="600" bind:value={draft.limits.max_per_minute} aria-label={t('settings.notify.perMinute')} aria-invalid={!!errors['limits.max_per_minute']} /> {t('settings.notify.perMin')}
        </div>
      </div>
      {#each ['limits.dedup_secs', 'limits.aggregate_secs', 'limits.max_per_minute'] as k (k)}
        {#if errors[k]}<p class="set-err pad">{errors[k]}</p>{/if}
      {/each}
    </section>

    {#if dirty}
      <div class="set-savebar" role="region" aria-label={t('settings.general.unsaved')}>
        <span>{invalid ? tp('settings.general.fixProblems', Object.keys(errors).length) : t('settings.general.unsaved')}</span>
        <button class="btn" disabled={saving} onclick={() => (draft = structuredClone($state.snapshot(saved)))}>{t('settings.general.discard')}</button>
        <button class="btn primary" disabled={saving || invalid} onclick={save}>{saving ? t('settings.general.saving') : t('common.save')}</button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 16px;
  }
  .head .text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
  }
  .grid {
    display: grid;
    grid-template-columns: minmax(0, 1.4fr) minmax(0, 1fr);
    gap: 12px;
  }
  .pw {
    display: flex;
    gap: 4px;
  }
  .pw .lib-input {
    flex: 1;
  }
  .rules .topic {
    width: 100%;
    min-width: 170px;
  }
  .rules .when {
    width: 158px;
    height: 28px;
  }
  .rules th,
  .rules td {
    padding-left: 6px;
    padding-right: 6px;
  }
  .rules th.c {
    font-size: 10px;
    letter-spacing: 0.01em;
  }
  .rules td input[type='checkbox'] {
    width: 15px;
    height: 15px;
    accent-color: var(--accent);
  }
  .rules tr.hit td {
    background: var(--accent-soft);
  }
  .rules tr.errrow td {
    padding-top: 0;
    border-bottom: 1px solid var(--border);
  }
  .rules tr:has(+ .errrow) td {
    border-bottom: 0;
  }
  .acts {
    white-space: nowrap;
  }
  .acts :global(.up) {
    transform: rotate(180deg);
  }
  .probe {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-left: auto;
  }
  .probe .lib-input {
    width: 170px;
    height: 28px;
  }
  .pad {
    padding: 0 16px 10px;
  }
  .sr {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
  @media (max-width: 720px) {
    .grid {
      grid-template-columns: minmax(0, 1fr);
    }
    .probe {
      margin-left: 0;
    }
  }
</style>
