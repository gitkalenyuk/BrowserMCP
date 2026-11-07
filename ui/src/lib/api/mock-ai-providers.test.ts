import { describe, expect, it } from 'vitest';
import { createProviderMocks } from './mock-ai-providers';

describe('provider mocks (follow crates/ai)', () => {
  it('adds, edits and removes user providers after the built-ins', async () => {
    const { handlers: h } = createProviderMocks();
    const added = await h['ai.provider_add']({ name: 'OpenAI (work)', template: 'openai', api_key: 'sk' });
    expect(added).toMatchObject({ id: 'openai-work', builtin: false, template: 'openai', configured: true, base_url: 'https://api.openai.com/v1' });
    const { providers } = await h['ai.providers']();
    expect(providers.at(-1)?.id).toBe('openai-work');
    expect(providers.findIndex((p) => p.builtin === false)).toBeGreaterThan(providers.findIndex((p) => p.id === 'lmstudio'));

    expect(() => h['ai.provider_add']({ name: 'openai (WORK)', template: 'openai' })).toThrow(/already exists/);
    expect(() => h['ai.provider_add']({ name: 'Box' })).toThrow(/base_url is required/);
    expect(() => h['ai.provider_add']({ name: 'Box', base_url: 'localhost:1' })).toThrow(/bad base URL/);

    const renamed = await h['ai.provider_set']({ id: 'openai-work', name: 'Work', headers: { 'X-A': 'b' }, default: true });
    expect(renamed).toMatchObject({ name: 'Work', headers: { 'X-A': 'b' }, is_default: true });
    expect(() => h['ai.provider_set']({ id: 'openai', name: 'Mine' })).toThrow(/built-in/);
    const cleared = await h['ai.provider_set']({ id: 'openai-work', base_url: '' });
    expect(cleared.configured).toBe(false);

    expect(() => h['ai.provider_remove']({ id: 'openai' })).toThrow(/cannot be removed/);
    expect(await h['ai.provider_remove']({ id: 'openai-work' })).toEqual({ id: 'openai-work', was_default: true });
    expect((await h['ai.providers']()).providers.some((p) => p.id === 'openai-work')).toBe(false);
  });

  it('test says what is missing', async () => {
    const { handlers: h } = createProviderMocks();
    await h['ai.provider_add']({ name: 'Groq 2', template: 'groq' });
    expect(await h['ai.provider_test']({ id: 'groq-2' })).toMatchObject({ ok: false, error: 'no API key set' });
  });
});
