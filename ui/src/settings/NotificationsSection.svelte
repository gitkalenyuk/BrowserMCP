<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { NotifyChannel, NotifyConfig } from '../lib/api/types-settings';
  import Icon from '../lib/components/Icon.svelte';
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
      notify('Notification settings saved');
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
      results = { ...results, [channel]: { ok: r.ok, text: r.ok ? 'Sent. Check that it arrived.' : (r.error ?? 'Failed') } };
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
  <button class="btn" disabled={testing !== null || saving || (dirty && invalid)} onclick={() => test(channel)} title={dirty ? 'Saves your changes, then sends a test' : 'Send a test notification'}>
    {testing === channel ? 'Sending…' : dirty ? 'Save & test' : 'Test'}
  </button>
{/snippet}
{#snippet result(channel: NotifyChannel)}
  {#if results[channel]}
    <p class={results[channel].ok ? 'set-ok' : 'set-err'} role="status">{results[channel].text}</p>
  {/if}
{/snippet}

<div class="set-page">
  <header>
    <h2>Notifications</h2>
    <p>
      Where finished runs, patch proposals, agents and downloads tell you about themselves. Questions (a run waiting for a
      patch) come with buttons, in the browser and on Telegram; the first answer wins. Saved in
      <span class="set-mono">data/config/notify.json</span>.
    </p>
  </header>

  {#if loadError}
    <p class="lib-error" role="alert">Could not load the notification settings: {loadError}</p>
    <button class="btn" onclick={load}>Retry</button>
  {:else if !draft || !tg || !wh}
    <p class="set-muted">Loading…</p>
  {:else}
    <section class="set-card" aria-labelledby="n-ch">
      <h3 id="n-ch">Channels</h3>
      <div class="set-row">
        <div class="text">
          <span class="label">Browser</span>
          <span class="help">A toast in the browser window.</span>
          {@render result('ui')}
        </div>
        <div class="ctl">
          {@render testButton('ui')}
          <Toggle label="Browser notifications" checked={draft.channels.ui} onchange={(on) => (draft!.channels.ui = on)} />
        </div>
      </div>
      <div class="set-row">
        <div class="text">
          <span class="label">Windows</span>
          <span class="help">A Windows notification, also when the browser is in the background.</span>
          {@render result('os')}
        </div>
        <div class="ctl">
          {@render testButton('os')}
          <Toggle label="Windows notifications" checked={draft.channels.os} onchange={(on) => (draft!.channels.os = on)} />
        </div>
      </div>

      <div class="set-row stack">
        <div class="head">
          <div class="text">
            <span class="label">Telegram</span>
            <span class="help">
              A bot messages you, with Accept / Reject / Skip buttons for questions. Create one with
              <span class="set-mono">@BotFather</span> (<span class="set-mono">/newbot</span>), send it any message, then use your chat id
              (for example from <span class="set-mono">@userinfobot</span>). Presses from other chats are ignored.
            </span>
          </div>
          <div class="ctl">
            {@render testButton('telegram')}
            <Toggle label="Telegram notifications" checked={tg.enabled} onchange={(on) => (tg.enabled = on)} />
          </div>
        </div>
        <div class="grid">
          <label class="lib-field">
            Bot token
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
              <button class="icon-btn" type="button" aria-label={showToken ? 'Hide token' : 'Show token'} onclick={() => (showToken = !showToken)}>
                <Icon name={showToken ? 'eye-off' : 'eye'} size={14} />
              </button>
            </span>
            {#if errors['telegram.bot_token']}<span class="set-err">{errors['telegram.bot_token']}</span>
            {:else if isMaskedToken(tg.bot_token)}<span class="set-muted">A token is stored (only its end is shown). Type a new one to replace it.</span>{/if}
          </label>
          <label class="lib-field">
            Chat id
            <input class="lib-input mono" spellcheck="false" bind:value={tg.chat_id} placeholder="123456789" aria-invalid={!!errors['telegram.chat_id']} />
            {#if errors['telegram.chat_id']}<span class="set-err">{errors['telegram.chat_id']}</span>{/if}
          </label>
        </div>
        {@render result('telegram')}
      </div>

      <div class="set-row stack">
        <div class="head">
          <div class="text">
            <span class="label">Webhook</span>
            <span class="help">
              <span class="set-mono">POST</span> with <span class="set-mono">{'{id, title, body, level, topic, data, actions, ts_ms}'}</span>
              for each notification (Slack-style relays, n8n, your own server).
            </span>
          </div>
          <div class="ctl">
            {@render testButton('webhook')}
            <Toggle label="Webhook notifications" checked={wh.enabled} onchange={(on) => (wh.enabled = on)} />
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
      <h3 id="n-rules"><span class="grow">Rules</span></h3>
      <p class="lead">
        Which events go where. The first rule whose topic matches (and whose condition holds) wins; channels switched off above are
        skipped. <b>Bundle</b> collects bursts (failed rows of a 1000-row run) into one message per minute.
      </p>
      <div class="set-table-wrap">
        <table class="set-table rules">
          <thead>
            <tr>
              <th class="r">#</th>
              <th>Event topic</th>
              {#each CHANNELS as c (c.id)}<th class="c">{c.label}</th>{/each}
              <th>When</th>
              <th class="c" title="Collect bursts into one summary">Bundle</th>
              <th><span class="sr">Order and delete</span></th>
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
                    aria-label="Event topic of rule {i + 1}"
                    aria-invalid={!!errors[`rule.${i}`]}
                    oninput={(e) => (draft!.rules = updateRule(draft!.rules, i, { topic: e.currentTarget.value }))}
                  />
                </td>
                {#each CHANNELS as c (c.id)}
                  <td class="c">
                    <input
                      type="checkbox"
                      checked={rule.channels.includes(c.id)}
                      aria-label="{c.label} for {rule.topic || `rule ${i + 1}`}"
                      onchange={() => (draft!.rules = toggleChannel(draft!.rules, i, c.id))}
                    />
                  </td>
                {/each}
                <td>
                  <select class="lib-input when" value={rule.when} aria-label="Condition of rule {i + 1}" onchange={(e) => (draft!.rules = updateRule(draft!.rules, i, { when: e.currentTarget.value as typeof rule.when }))}>
                    {#each WHEN as w (w.id)}<option value={w.id}>{w.label}</option>{/each}
                  </select>
                </td>
                <td class="c">
                  <input type="checkbox" checked={rule.aggregate} aria-label="Bundle bursts of rule {i + 1}" onchange={(e) => (draft!.rules = updateRule(draft!.rules, i, { aggregate: e.currentTarget.checked }))} />
                </td>
                <td class="acts">
                  <button class="icon-btn" aria-label="Move rule {i + 1} up" disabled={i === 0} onclick={() => (draft!.rules = moveRule(draft!.rules, i, -1))}><Icon name="chevron-down" size={13} class="up" /></button>
                  <button class="icon-btn" aria-label="Move rule {i + 1} down" disabled={i === draft.rules.length - 1} onclick={() => (draft!.rules = moveRule(draft!.rules, i, 1))}><Icon name="chevron-down" size={13} /></button>
                  <button class="icon-btn" aria-label="Delete rule {i + 1}" onclick={() => (draft!.rules = removeRule(draft!.rules, i))}><Icon name="trash" size={13} /></button>
                </td>
              </tr>
              {#if errors[`rule.${i}`]}
                <tr class="errrow"><td></td><td colspan={CHANNELS.length + 4}><span class="set-err">{errors[`rule.${i}`]}</span></td></tr>
              {/if}
            {:else}
              <tr><td colspan={CHANNELS.length + 5} class="set-muted">No rules: nothing is sent except tests and questions asked by agents.</td></tr>
            {/each}
          </tbody>
        </table>
        <datalist id="notify-topics">{#each KNOWN_TOPICS as t (t)}<option value={t}></option>{/each}</datalist>
      </div>
      <div class="set-actions">
        <button class="btn" onclick={() => (draft!.rules = addRule(draft!.rules))}><Icon name="plus" size={13} /> Add rule</button>
        {#if missing.length}
          <button class="btn" onclick={() => (draft!.rules = [...draft!.rules, ...missing])} title={missing.map((r) => r.topic).join(', ')}>
            Add {missing.length} missing default rule{missing.length === 1 ? '' : 's'}
          </button>
        {/if}
        <span class="probe">
          <label for="probe" class="set-muted">Which rule handles</label>
          <input id="probe" class="lib-input mono" bind:value={probe} list="notify-topics" spellcheck="false" />
          <span class="set-muted" role="status">{probe.trim() ? (probeHit >= 0 ? `→ rule ${probeHit + 1}` : '→ none (not sent)') : ''}</span>
        </span>
      </div>
    </section>

    <section class="set-card" aria-labelledby="n-lim">
      <h3 id="n-lim">Anti-spam</h3>
      <div class="set-row">
        <div class="text"><span class="label">Drop repeats</span><span class="help">The same title and text again within this many seconds is not sent.</span></div>
        <div class="ctl">
          <input class="lib-input narrow" type="number" min="0" max="3600" bind:value={draft.limits.dedup_secs} aria-label="Drop repeats, seconds" aria-invalid={!!errors['limits.dedup_secs']} /> s
        </div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">Bundle window</span><span class="help">How long bundled rules collect before sending a summary.</span></div>
        <div class="ctl">
          <input class="lib-input narrow" type="number" min="1" max="3600" bind:value={draft.limits.aggregate_secs} aria-label="Bundle window, seconds" aria-invalid={!!errors['limits.aggregate_secs']} /> s
        </div>
      </div>
      <div class="set-row">
        <div class="text"><span class="label">At most per minute</span><span class="help">Per channel. Extra ones are counted (“+3 more suppressed”); questions always go through.</span></div>
        <div class="ctl">
          <input class="lib-input narrow" type="number" min="1" max="600" bind:value={draft.limits.max_per_minute} aria-label="At most per minute" aria-invalid={!!errors['limits.max_per_minute']} /> /min
        </div>
      </div>
      {#each ['limits.dedup_secs', 'limits.aggregate_secs', 'limits.max_per_minute'] as k (k)}
        {#if errors[k]}<p class="set-err pad">{errors[k]}</p>{/if}
      {/each}
    </section>

    {#if dirty}
      <div class="set-savebar" role="region" aria-label="Unsaved changes">
        <span>{invalid ? `Fix ${Object.keys(errors).length} problem${Object.keys(errors).length === 1 ? '' : 's'} to save` : 'Unsaved changes'}</span>
        <button class="btn" disabled={saving} onclick={() => (draft = structuredClone($state.snapshot(saved)))}>Discard</button>
        <button class="btn primary" disabled={saving || invalid} onclick={save}>{saving ? 'Saving…' : 'Save'}</button>
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
