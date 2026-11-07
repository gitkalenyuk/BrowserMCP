import { describe, expect, it } from 'vitest';
import { chatTitle, defaultModelName, pickModel, rememberModel } from './prompt';

const offered = {
  models: [
    { id: 'opus', name: 'Opus' },
    { id: 'sonnet', name: 'Sonnet' },
  ],
  default: 'sonnet',
};

describe('new tab model picker', () => {
  it('preselects the remembered model while the provider offers it', () => {
    expect(pickModel({ 'claude-code': 'opus' }, 'claude-code', offered)).toBe('opus');
    expect(pickModel({ 'claude-code': 'gone' }, 'claude-code', offered)).toBe('');
    expect(pickModel({}, 'claude-code', offered)).toBe('');
    expect(pickModel({ 'claude-code': 'opus' }, 'claude-code', null)).toBe('');
  });

  it('remembers per provider', () => {
    let c = rememberModel({}, 'claude-code', 'opus');
    c = rememberModel(c, 'codex', 'gpt-5');
    expect(c).toEqual({ 'claude-code': 'opus', codex: 'gpt-5' });
    expect(rememberModel(c, 'codex', '')).toEqual({ 'claude-code': 'opus' });
  });

  it('names the default model', () => {
    expect(defaultModelName(offered)).toBe('Sonnet');
    expect(defaultModelName({ models: [], default: 'x-1' })).toBe('x-1');
    expect(defaultModelName({ models: [] })).toBeNull();
    expect(defaultModelName(null)).toBeNull();
  });
});

describe('started message', () => {
  it('uses the chat title, else the task, never the session id', () => {
    expect(chatTitle('Compare prices', 'Price comparison')).toBe('Price comparison');
    expect(chatTitle('  Compare   prices\nacross stores  ')).toBe('Compare prices');
    const long = 'word '.repeat(30);
    const title = chatTitle(long, null, 20);
    expect(title.length).toBeLessThanOrEqual(20);
    expect(title.endsWith('…')).toBe(true);
  });
});
