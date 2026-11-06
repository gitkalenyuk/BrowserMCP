import { describe, expect, it } from 'vitest';
import type { NotifyConfig, NotifyRule } from '../api/types-settings';
import {
  DEFAULT_RULES,
  addRule,
  botTokenError,
  chatIdError,
  configErrors,
  configPatch,
  firstMatch,
  isEmptyPatch,
  missingDefaults,
  moveRule,
  removeRule,
  ruleErrors,
  toggleChannel,
  topicError,
  topicMatches,
  updateRule,
  webhookUrlError,
} from './notify';

function config(): NotifyConfig {
  return {
    channels: {
      ui: true,
      os: true,
      telegram: { enabled: false, bot_token: '…wxyz', chat_id: '', api_base: 'https://api.telegram.org' },
      webhook: { enabled: false, url: '' },
    },
    rules: structuredClone(DEFAULT_RULES),
    limits: { dedup_secs: 10, aggregate_secs: 60, max_per_minute: 20 },
  };
}

describe('rules table edits', () => {
  it('adds, moves, updates and removes rules', () => {
    let rules = DEFAULT_RULES.slice(0, 2);
    rules = addRule(rules, 'net.*');
    expect(rules.map((r) => r.topic)).toEqual(['run.finished', 'run.heal_needed', 'net.*']);
    expect(rules[2]).toEqual({ topic: 'net.*', channels: ['ui'], when: 'always', aggregate: false });

    rules = moveRule(rules, 2, -1);
    expect(rules.map((r) => r.topic)).toEqual(['run.finished', 'net.*', 'run.heal_needed']);
    expect(moveRule(rules, 0, -1)).toBe(rules); // already first
    expect(moveRule(rules, 2, 1)).toBe(rules); // already last

    rules = updateRule(rules, 1, { when: 'failed', aggregate: true });
    expect(rules[1]).toMatchObject({ topic: 'net.*', when: 'failed', aggregate: true });

    rules = removeRule(rules, 0);
    expect(rules.map((r) => r.topic)).toEqual(['net.*', 'run.heal_needed']);
    // The inputs are never mutated.
    expect(DEFAULT_RULES[0].topic).toBe('run.finished');
  });

  it('toggles channels and keeps them in the canonical order', () => {
    let rules: NotifyRule[] = [{ topic: 'x', channels: ['webhook'], when: 'always', aggregate: false }];
    rules = toggleChannel(rules, 0, 'ui');
    rules = toggleChannel(rules, 0, 'telegram');
    expect(rules[0].channels).toEqual(['ui', 'telegram', 'webhook']);
    rules = toggleChannel(rules, 0, 'ui');
    expect(rules[0].channels).toEqual(['telegram', 'webhook']);
  });

  it('matches topics like crates/notify and finds the rule that wins', () => {
    expect(topicMatches('run.*', 'run.finished')).toBe(true);
    expect(topicMatches('run.*', 'runs.updated')).toBe(false);
    expect(topicMatches('run.*', 'run')).toBe(false);
    expect(topicMatches('*', 'anything')).toBe(true);
    expect(topicMatches(' run.finished ', 'run.finished')).toBe(true);
    const rules = [...DEFAULT_RULES, { topic: 'run.*', channels: ['ui' as const], when: 'always' as const, aggregate: false }];
    expect(firstMatch(rules, 'run.heal_proposed')).toBe(2);
    expect(firstMatch(rules, 'run.started')).toBe(rules.length - 1);
    expect(firstMatch(rules, 'tab.updated')).toBe(-1);
  });

  it('validates topics, channels and unreachable rules', () => {
    expect(topicError('')).toMatch(/Enter/);
    expect(topicError('Run.Finished')).toMatch(/exact topic/);
    expect(topicError('run.')).not.toBeNull();
    expect(topicError('run.*')).toBeNull();
    expect(topicError('*')).toBeNull();

    const errors = ruleErrors([
      { topic: 'run.*', channels: ['ui'], when: 'always', aggregate: false },
      { topic: 'run.finished', channels: ['os'], when: 'always', aggregate: false },
      { topic: 'ai.done', channels: [], when: 'always', aggregate: false },
      { topic: 'ai.*', channels: ['ui'], when: 'failed', aggregate: false },
      { topic: 'ai.session', channels: ['ui'], when: 'always', aggregate: false },
    ]);
    expect(errors.get(0)).toBeUndefined();
    expect(errors.get(1)).toMatch(/Never used: rule 1 \(run\.\*\)/);
    expect(errors.get(2)).toMatch(/channel/);
    // A conditional rule does not hide later ones.
    expect(errors.get(4)).toBeUndefined();
    expect(ruleErrors(DEFAULT_RULES).size).toBe(0);
  });

  it('lists default rules missing from an older config', () => {
    const old = DEFAULT_RULES.filter((r) => !r.topic.startsWith('run.heal_p') && r.topic !== 'run.heal_applied' && r.topic !== 'run.heal_failed');
    expect(missingDefaults(old).map((r) => r.topic)).toEqual(['run.heal_proposed', 'run.heal_applied', 'run.heal_failed']);
    expect(missingDefaults(DEFAULT_RULES)).toEqual([]);
  });
});

describe('channel form validation', () => {
  it('checks the Telegram token and chat id', () => {
    expect(botTokenError('', false)).toBeNull();
    expect(botTokenError('', true)).toMatch(/BotFather/);
    expect(botTokenError('…wxyz', true)).toBeNull(); // masked: keeps the stored token
    expect(botTokenError('123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw', true)).toBeNull();
    expect(botTokenError('not a token', true)).toMatch(/looks like/);
    expect(chatIdError('', false)).toBeNull();
    expect(chatIdError('', true)).not.toBeNull();
    expect(chatIdError('123456789', true)).toBeNull();
    expect(chatIdError('-1001234567890', true)).toBeNull();
    expect(chatIdError('@my_channel', true)).toBeNull();
    expect(chatIdError('my chat', true)).toMatch(/number/);
  });

  it('checks the webhook URL', () => {
    expect(webhookUrlError('', false)).toBeNull();
    expect(webhookUrlError('', true)).not.toBeNull();
    expect(webhookUrlError('https://hooks.example.com/x', true)).toBeNull();
    expect(webhookUrlError('ftp://example.com', true)).toMatch(/http/);
    expect(webhookUrlError('example.com/hook', true)).toMatch(/valid/);
  });

  it('collects every problem of a draft', () => {
    const c = config();
    expect(configErrors(c)).toEqual({});
    c.channels.telegram.enabled = true;
    c.channels.webhook.enabled = true;
    c.limits.max_per_minute = 0;
    c.rules = addRule(c.rules, '');
    const errors = configErrors(c);
    expect(Object.keys(errors).sort()).toEqual(['limits.max_per_minute', `rule.${c.rules.length - 1}`, 'telegram.chat_id', 'webhook.url']);
  });
});

describe('configPatch', () => {
  it('is empty when nothing changed', () => {
    expect(isEmptyPatch(configPatch(config(), config()))).toBe(true);
  });

  it('sends only what changed, trimmed; the masked token is never sent back', () => {
    const saved = config();
    const draft = config();
    draft.channels.os = false;
    draft.channels.telegram.enabled = true;
    draft.channels.telegram.chat_id = ' 42 ';
    draft.channels.webhook.url = 'https://example.test/h ';
    draft.limits.dedup_secs = 30;
    expect(configPatch(saved, draft)).toEqual({
      os: false,
      telegram: { enabled: true, chat_id: '42' },
      webhook: { url: 'https://example.test/h' },
      limits: { dedup_secs: 30, aggregate_secs: 60, max_per_minute: 20 },
    });
    draft.channels.telegram.bot_token = '123:new';
    draft.rules = moveRule(draft.rules, 1, -1);
    const p = configPatch(saved, draft);
    expect(p.telegram?.bot_token).toBe('123:new');
    expect(p.rules?.[0].topic).toBe('run.heal_needed');
  });
});
