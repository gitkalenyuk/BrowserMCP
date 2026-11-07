// Notifications settings: the per-topic rules table and the channel forms (crates/notify,
// data/config/notify.json). Pure helpers, unit-tested; the page keeps a draft and saves only
// what changed with `notify.config_set`.

import type { NotifyChannel, NotifyConfig, NotifyConfigPatch, NotifyRule, NotifyWhen } from '../api/types-settings';
// `translate`: several helpers below use `t` for the trimmed topic / token.
import { t as translate } from '../i18n/index.svelte';

// Labels are getters: translate() runs when markup reads them, so they follow the language.

export const CHANNELS: { id: NotifyChannel; label: string }[] = [
  { id: 'ui', get label() { return translate('settings.notify.channel.ui'); } },
  { id: 'os', get label() { return translate('settings.notify.channel.os'); } },
  { id: 'telegram', get label() { return translate('settings.notify.channel.telegram'); } },
  { id: 'webhook', get label() { return translate('settings.notify.channel.webhook'); } },
];

export const WHEN: { id: NotifyWhen; label: string }[] = [
  { id: 'always', get label() { return translate('settings.notify.when.always'); } },
  { id: 'unfocused', get label() { return translate('settings.notify.when.unfocused'); } },
  { id: 'failed', get label() { return translate('settings.notify.when.failed'); } },
];

const all: NotifyChannel[] = ['ui', 'os', 'telegram', 'webhook'];
const rule = (topic: string, channels: NotifyChannel[], when: NotifyWhen = 'always', aggregate = false): NotifyRule => ({
  topic,
  channels,
  when,
  aggregate,
});

/** The rules a fresh notify.json gets (mirror of `NotifyConfig::default` in crates/notify). */
export const DEFAULT_RULES: NotifyRule[] = [
  rule('run.finished', all),
  rule('run.heal_needed', all),
  rule('run.heal_proposed', all),
  rule('run.heal_applied', ['ui', 'telegram']),
  rule('run.heal_failed', all),
  rule('run.row_failed', ['ui'], 'always', true),
  rule('ai.done', ['os', 'telegram'], 'unfocused'),
  rule('download.done', ['ui', 'os']),
  rule('trigger.fired', all, 'failed'),
];

/** Topics worth suggesting in the rule editor (what the core emits). */
export const KNOWN_TOPICS = [
  'run.finished',
  'run.heal_needed',
  'run.heal_proposed',
  'run.heal_applied',
  'run.heal_failed',
  'run.row_failed',
  'run.*',
  'ai.done',
  'ai.session',
  'download.done',
  'download.failed',
  'trigger.fired',
  'net.paused',
  '*',
];

// ---- the rules table -------------------------------------------------------------------------

export function addRule(rules: NotifyRule[], topic = ''): NotifyRule[] {
  return [...rules, rule(topic, ['ui'])];
}

export function removeRule(rules: NotifyRule[], index: number): NotifyRule[] {
  return rules.filter((_, i) => i !== index);
}

/** Move a rule up (-1) or down (+1); order matters, the first match wins. */
export function moveRule(rules: NotifyRule[], index: number, by: -1 | 1): NotifyRule[] {
  const to = index + by;
  if (index < 0 || index >= rules.length || to < 0 || to >= rules.length) return rules;
  const next = [...rules];
  [next[index], next[to]] = [next[to], next[index]];
  return next;
}

export function updateRule(rules: NotifyRule[], index: number, patch: Partial<NotifyRule>): NotifyRule[] {
  return rules.map((r, i) => (i === index ? { ...r, ...patch } : r));
}

/** Switch one channel of a rule on or off, keeping the channels in the canonical order. */
export function toggleChannel(rules: NotifyRule[], index: number, channel: NotifyChannel): NotifyRule[] {
  return rules.map((r, i) => {
    if (i !== index) return r;
    const on = new Set(r.channels);
    if (on.has(channel)) on.delete(channel);
    else on.add(channel);
    return { ...r, channels: all.filter((c) => on.has(c)) };
  });
}

/** `run.finished` matches `run.finished`, `run.*` and `*` (same as crates/notify/src/rules.rs). */
export function topicMatches(pattern: string, topic: string): boolean {
  const p = pattern.trim();
  if (p === '*') return true;
  if (p.endsWith('.*')) {
    const prefix = p.slice(0, -2);
    return topic.startsWith(prefix) && topic.slice(prefix.length).startsWith('.');
  }
  return p === topic;
}

/** Index of the rule an event with `topic` hits first (ignoring `when`), or -1. */
export function firstMatch(rules: NotifyRule[], topic: string): number {
  return rules.findIndex((r) => topicMatches(r.topic, topic.trim()));
}

const TOPIC_RE = /^(\*|[a-z0-9_]+(\.[a-z0-9_]+)*(\.\*)?)$/;

export function topicError(topic: string): string | null {
  const t = topic.trim();
  if (!t) return translate('settings.notify.err.topicEmpty');
  if (!TOPIC_RE.test(t)) return translate('settings.notify.err.topic');
  return null;
}

/** Problems per rule index: bad topic, no channel, or never reached behind an earlier rule. */
export function ruleErrors(rules: NotifyRule[]): Map<number, string> {
  const errors = new Map<number, string>();
  rules.forEach((r, i) => {
    const bad = topicError(r.topic);
    if (bad) return errors.set(i, bad);
    if (r.channels.length === 0) return errors.set(i, translate('settings.notify.err.noChannel'));
    // An earlier rule with `when: always` that matches every topic this one matches hides it.
    const shadow = rules
      .slice(0, i)
      .findIndex((e) => e.when === 'always' && !topicError(e.topic) && covers(e.topic.trim(), r.topic.trim()));
    if (shadow >= 0) errors.set(i, translate('settings.notify.err.shadowed', { n: shadow + 1, topic: rules[shadow].topic.trim() }));
  });
  return errors;
}

/** Does pattern `a` match every topic pattern `b` matches? */
function covers(a: string, b: string): boolean {
  if (a === '*') return true;
  if (b === '*') return false;
  if (a.endsWith('.*')) {
    const prefix = a.slice(0, -2);
    const bBase = b.endsWith('.*') ? b.slice(0, -2) : b;
    return bBase.startsWith(prefix + '.');
  }
  return a === b;
}

/** Default rules that are missing from `rules` (by topic), e.g. after an upgrade. */
export function missingDefaults(rules: NotifyRule[]): NotifyRule[] {
  const have = new Set(rules.map((r) => r.topic.trim()));
  return DEFAULT_RULES.filter((r) => !have.has(r.topic));
}

// ---- channel forms ---------------------------------------------------------------------------

/** The masked token `notify.config_get` returns (`…abcd`); sending it back keeps the stored one. */
export function isMaskedToken(token: string): boolean {
  return token.startsWith('…');
}

export function botTokenError(token: string, enabled: boolean): string | null {
  const t = token.trim();
  if (!t) return enabled ? translate('settings.notify.err.tokenEmpty') : null;
  if (isMaskedToken(t)) return null;
  if (!/^\d{5,}:[A-Za-z0-9_-]{20,}$/.test(t)) return translate('settings.notify.err.token');
  return null;
}

export function chatIdError(chat: string, enabled: boolean): string | null {
  const c = chat.trim();
  if (!c) return enabled ? translate('settings.notify.err.chatEmpty') : null;
  if (/^-?\d{1,20}$/.test(c) || /^@[A-Za-z0-9_]{5,32}$/.test(c)) return null;
  return translate('settings.notify.err.chat');
}

export function webhookUrlError(url: string, enabled: boolean): string | null {
  const u = url.trim();
  if (!u) return enabled ? translate('settings.notify.err.urlEmpty') : null;
  try {
    const parsed = new URL(u);
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return translate('settings.notify.err.urlScheme');
    return null;
  } catch {
    return translate('settings.notify.err.url');
  }
}

export function limitError(value: number, min: number, max: number): string | null {
  if (!Number.isFinite(value) || !Number.isInteger(value)) return translate('settings.notify.err.whole');
  if (value < min || value > max) return translate('settings.notify.err.between', { min, max });
  return null;
}

/** Every form problem, keyed by field (`telegram.bot_token`, `rule.2`, …). Empty = valid. */
export function configErrors(c: NotifyConfig): Record<string, string> {
  const out: Record<string, string> = {};
  const set = (k: string, e: string | null) => {
    if (e) out[k] = e;
  };
  const tg = c.channels.telegram;
  set('telegram.bot_token', botTokenError(tg.bot_token, tg.enabled));
  set('telegram.chat_id', chatIdError(tg.chat_id, tg.enabled));
  set('webhook.url', webhookUrlError(c.channels.webhook.url, c.channels.webhook.enabled));
  set('limits.dedup_secs', limitError(c.limits.dedup_secs, 0, 3600));
  set('limits.aggregate_secs', limitError(c.limits.aggregate_secs, 1, 3600));
  set('limits.max_per_minute', limitError(c.limits.max_per_minute, 1, 600));
  for (const [i, e] of ruleErrors(c.rules)) out[`rule.${i}`] = e;
  return out;
}

const sameJson = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/** `notify.config_set` arguments with only what differs between `saved` and `draft`. */
export function configPatch(saved: NotifyConfig, draft: NotifyConfig): NotifyConfigPatch {
  const patch: NotifyConfigPatch = {};
  if (saved.channels.ui !== draft.channels.ui) patch.ui = draft.channels.ui;
  if (saved.channels.os !== draft.channels.os) patch.os = draft.channels.os;
  const a = saved.channels.telegram;
  const b = draft.channels.telegram;
  const tg: NonNullable<NotifyConfigPatch['telegram']> = {};
  if (a.enabled !== b.enabled) tg.enabled = b.enabled;
  if (a.bot_token !== b.bot_token.trim()) tg.bot_token = b.bot_token.trim();
  if (a.chat_id !== b.chat_id.trim()) tg.chat_id = b.chat_id.trim();
  if (a.api_base !== b.api_base.trim()) tg.api_base = b.api_base.trim();
  if (Object.keys(tg).length) patch.telegram = tg;
  const wa = saved.channels.webhook;
  const wb = draft.channels.webhook;
  const wh: NonNullable<NotifyConfigPatch['webhook']> = {};
  if (wa.enabled !== wb.enabled) wh.enabled = wb.enabled;
  if (wa.url !== wb.url.trim()) wh.url = wb.url.trim();
  if (Object.keys(wh).length) patch.webhook = wh;
  const rules = draft.rules.map((r) => ({ ...r, topic: r.topic.trim() }));
  if (!sameJson(saved.rules, rules)) patch.rules = rules;
  if (!sameJson(saved.limits, draft.limits)) patch.limits = { ...draft.limits };
  return patch;
}

export function isEmptyPatch(p: object): boolean {
  return Object.keys(p).length === 0;
}
