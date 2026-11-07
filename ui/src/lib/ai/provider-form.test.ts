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
    model_set: false,
    header_names: [],
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

  it('saved headers: values stay hidden, null keeps them, {} removes them all', () => {
    const p = provider({ header_names: ['HTTP-Referer'] });
    const form = formFrom(p);
    expect(form.headers).toEqual([{ name: 'HTTP-Referer', value: '', stored: true }]);
    expect(isDirty(form, formFrom(p), p)).toBe(false);
    // Adding a header keeps the saved one's unknown value.
    form.headers.push({ name: 'X-Title', value: 'Mine' });
    expect(changes(form, formFrom(p), p)).toEqual({ id: 'openai', headers: { 'HTTP-Referer': null, 'X-Title': 'Mine' } });
    // Typing into a saved header replaces its value.
    form.headers = [{ name: 'HTTP-Referer', value: ' https://me.test ', stored: true }];
    expect(changes(form, formFrom(p), p)).toEqual({ id: 'openai', headers: { 'HTTP-Referer': 'https://me.test' } });
    form.headers = [];
    expect(changes(form, formFrom(p), p)).toEqual({ id: 'openai', headers: {} });
  });

  it('a model the user never chose shows as "Provider default"', () => {
    const auto = provider({ model: 'gpt-5', models: ['gpt-5', 'gpt-5-mini'] });
    const form = formFrom(auto);
    expect(form.model).toBe('');
    expect(isDirty(form, formFrom(auto), auto)).toBe(false);
    form.model = 'gpt-5-mini';
    expect(changes(form, formFrom(auto), auto)).toEqual({ id: 'openai', model: 'gpt-5-mini' });
    // A chosen one shows as itself; picking "Provider default" clears it.
    const chosen = { ...auto, model: 'gpt-5-mini', model_set: true };
    const again = formFrom(chosen);
    expect(again.model).toBe('gpt-5-mini');
    again.model = '';
    expect(changes(again, formFrom(chosen), chosen)).toEqual({ id: 'openai', model: '' });
  });

  it('follows the provider when it changes underneath, keeping edits', () => {
    const saved = formFrom(user);
    const form = { ...formFrom(user), name: 'Edited', apiKey: 'k' };
    // Saved elsewhere: a new header and URL; the user never touched them here.
    const tested = { ...user, base_url: 'http://gpu:11434/v1', header_names: ['X-Team'], models: ['qwen3:8b'] };
    const fresh = formFrom(tested);
    const next = rebase(form, saved, fresh);
    expect(next).toMatchObject({ name: 'Edited', apiKey: 'k', baseUrl: 'http://gpu:11434/v1', headers: [{ name: 'X-Team', value: '', stored: true }] });
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
    // The core accepts visible ASCII only (reqwest's HeaderValue).
    expect(validate({ ...form, headers: [{ name: 'X-Title', value: 'Мій браузер' }] }, opts).headers).toBe('aiProviders.error.headerAscii');
    expect(validate({ ...form, headers: [{ name: 'X-Title', value: 'a\nb' }] }, opts).headers).toBe('aiProviders.error.headerAscii');
    expect(validate({ ...form, headers: [{ name: 'api-key', value: '', stored: true }] }, opts)).toEqual({});
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
    expect(headerMap([{ name: 'A', value: ' ', stored: true }])).toEqual({ A: null });
  });
});
