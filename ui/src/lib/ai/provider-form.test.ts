import { describe, expect, it } from 'vitest';
import type { AiProvider } from '../api';
import { addArgs, changes, formFrom, formFromTemplate, freeName, headerMap, isDirty, isDraftDirty, isHttpUrl, rebase, validate } from './provider-form';

function provider(over: Partial<AiProvider> = {}): AiProvider {
  return {
    id: 'openai',
    name: 'OpenAI',
    kind: 'api',
    protocol: 'openai',
    configured: false,
    needs_key: true,
    has_key: false,
    base_url: 'https://api.openai.com/v1',
    model: null,
    models: [],
    path: null,
    builtin: true,
    template: null,
    headers: {},
    ...over,
  };
}

const user = provider({ id: 'lab', name: 'Lab', builtin: false, template: 'ollama', base_url: 'http://localhost:11434/v1', needs_key: false });

describe('provider form', () => {
  it('starts clean and sends only what changed', () => {
    const saved = formFrom(user);
    const form = formFrom(user);
    expect(isDirty(form, saved, user)).toBe(false);
    expect(changes(form, saved, user)).toEqual({ id: 'lab' });

    form.name = ' Lab GPU ';
    form.api = 'anthropic';
    form.apiKey = ' sk-1 ';
    form.headers = [
      { name: ' X-Team ', value: ' lab ' },
      { name: '', value: '' },
    ];
    expect(isDirty(form, saved, user)).toBe(true);
    expect(changes(form, saved, user)).toEqual({ id: 'lab', name: 'Lab GPU', api: 'anthropic', api_key: 'sk-1', headers: { 'X-Team': 'lab' } });
  });

  it('never sends a name or API type for built-ins, nor a path for API providers', () => {
    const p = provider();
    const form = formFrom(p);
    form.name = 'Mine';
    form.api = 'gemini';
    form.path = 'C:\\x.exe';
    expect(changes(form, formFrom(p), p)).toEqual({ id: 'openai' });
    expect(isDirty(form, formFrom(p), p)).toBe(false);
  });

  it('removing every header sends {}', () => {
    const p = provider({ headers: { 'HTTP-Referer': 'https://me.test' } });
    const form = formFrom(p);
    expect(form.headers).toEqual([{ name: 'HTTP-Referer', value: 'https://me.test' }]);
    form.headers = [];
    expect(changes(form, formFrom(p), p)).toEqual({ id: 'openai', headers: {} });
  });

  it('follows the provider when it changes underneath, keeping edits', () => {
    const saved = formFrom(user);
    const form = { ...formFrom(user), name: 'Edited', apiKey: 'k' };
    // A test loaded models: the resolved default model changed; the user never touched it.
    const tested = { ...user, model: 'qwen3:8b', models: ['qwen3:8b'] };
    const fresh = formFrom(tested);
    const next = rebase(form, saved, fresh);
    expect(next).toMatchObject({ name: 'Edited', apiKey: 'k', model: 'qwen3:8b' });
    expect(changes(next, fresh, tested)).toEqual({ id: 'lab', name: 'Edited', api_key: 'k' });
    // Without edits nothing is dirty after the change.
    expect(isDirty(rebase(formFrom(user), saved, fresh), fresh, tested)).toBe(false);
  });

  it('a new provider copies its template and gets a free name', () => {
    const ollama = provider({ id: 'ollama', name: 'Ollama', base_url: 'http://localhost:11434/v1', needs_key: false });
    const form = formFromTemplate(ollama, ['Ollama', 'ollama 2']);
    expect(form).toMatchObject({ name: 'Ollama 3', api: 'openai', baseUrl: 'http://localhost:11434/v1' });
    expect(isDraftDirty(form, { ...form })).toBe(false);
    expect(isDraftDirty({ ...form, apiKey: 'k' }, form)).toBe(true);
    expect(addArgs({ ...form, model: ' qwen3 ' }, 'ollama')).toEqual({
      name: 'Ollama 3',
      template: 'ollama',
      api: 'openai',
      base_url: 'http://localhost:11434/v1',
      model: 'qwen3',
    });
    const blank = formFromTemplate(null, []);
    expect(blank).toMatchObject({ name: '', baseUrl: '', api: 'openai' });
    expect(addArgs({ ...blank, name: 'Box', baseUrl: 'http://box:8000/v1' }, null).template).toBe('custom');
    expect(freeName('A', [])).toBe('A');
  });

  it('checks names, URLs and headers', () => {
    const opts = { kind: 'api' as const, user: true, taken: ['OpenAI', 'Other'] };
    const form = formFrom(user);
    expect(validate(form, opts)).toEqual({});
    expect(validate({ ...form, name: ' ' }, opts).name).toBe('aiProviders.error.nameEmpty');
    expect(validate({ ...form, name: 'other' }, opts).name).toBe('aiProviders.error.nameTaken');
    expect(validate({ ...form, name: 'x'.repeat(65) }, opts).name).toBe('aiProviders.error.nameLong');
    expect(validate({ ...form, baseUrl: '' }, opts).baseUrl).toBe('aiProviders.error.urlEmpty');
    expect(validate({ ...form, baseUrl: 'localhost:8000' }, opts).baseUrl).toBe('aiProviders.error.urlBad');
    expect(validate({ ...form, headers: [{ name: 'Bad Name', value: 'x' }] }, opts).headers).toBe('aiProviders.error.header');
    expect(validate({ ...form, headers: [{ name: '', value: 'orphan' }] }, opts).headers).toBe('aiProviders.error.header');
    // A built-in may clear its URL (back to the preset) and keeps its fixed name.
    expect(validate({ ...formFrom(provider()), baseUrl: '' }, { ...opts, user: false })).toEqual({});
    // CLI providers have no URL.
    expect(validate({ ...form, baseUrl: '' }, { kind: 'cli', user: false, taken: [] })).toEqual({});
  });

  it('helpers', () => {
    expect(isHttpUrl('https://api.x.ai/v1')).toBe(true);
    expect(isHttpUrl('ftp://host')).toBe(false);
    expect(isHttpUrl('nope')).toBe(false);
    expect(headerMap([{ name: 'A', value: '1' }, { name: 'A', value: '2' }])).toEqual({ A: '2' });
  });
});
