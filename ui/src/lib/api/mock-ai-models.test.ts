// The calls the new tab page makes, against the mock: the model picker (ai.models →
// ai.session_start {model}) and the Agents & runs widget (run.list, run.heal_proposals).
import { afterEach, describe, expect, it } from 'vitest';
import { MockApi } from './mock';

let api: MockApi;
afterEach(() => api?.close());

describe('new tab calls in the mock', () => {
  it('lists models per provider and starts a chat with the chosen one', async () => {
    api = new MockApi({ simulate: false });
    const claude = await api.call('ai.models', { provider: 'claude-code' });
    expect(claude.models.map((m) => m.id)).toEqual(expect.arrayContaining(['default', 'opus', 'sonnet', 'haiku']));
    expect(claude.default).toBe('default');
    const anthropic = await api.call('ai.models', { provider: 'anthropic' });
    expect(anthropic.models.length).toBeGreaterThan(0);
    await expect(api.call('ai.models', { provider: 'nope' })).rejects.toThrow();

    const session = await api.call('ai.session_start', { provider: 'claude-code', prompt: 'Compare prices', model: 'opus' });
    expect(session.model).toBe('opus');
  });

  it('has the runs and heal proposals the Agents & runs widget reads', async () => {
    api = new MockApi({ simulate: false });
    const { runs } = await api.call('run.list', { limit: 20 });
    expect(runs.length).toBeGreaterThan(0);
    for (const r of runs) {
      expect(typeof r.script).toBe('string');
      expect(r.counts.failed).toBeGreaterThanOrEqual(0);
    }
    const { proposals } = await api.call('run.heal_proposals', {});
    expect(Array.isArray(proposals)).toBe(true);
  });
});
