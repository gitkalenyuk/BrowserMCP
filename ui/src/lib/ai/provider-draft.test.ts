import { afterEach, describe, expect, it } from 'vitest';
import { draftFor, dropDraft, keepDraft, NEW_DRAFT, peekDraft } from './provider-draft';
import type { ProviderForm } from './provider-form';

const form = (over: Partial<ProviderForm> = {}): ProviderForm => ({ name: 'Lab', api: 'openai', baseUrl: 'http://lab:8000/v1', apiKey: '', model: '', path: '', headers: [], ...over });

describe('provider draft (unsaved edits across remounts)', () => {
  afterEach(() => dropDraft());

  it('keeps the edits of one form until they are saved or discarded', () => {
    expect(peekDraft()).toBeNull();
    const edited = form({ apiKey: 'sk-typed' });
    keepDraft({ key: 'lab', form: edited, saved: form() });
    // A copy: later edits of the live form do not leak into the kept draft.
    edited.apiKey = 'changed';
    expect(draftFor('lab')?.form.apiKey).toBe('sk-typed');
    expect(draftFor('other')).toBeNull();
    expect(peekDraft()?.key).toBe('lab');

    dropDraft('other');
    expect(draftFor('lab')).not.toBeNull();
    dropDraft('lab');
    expect(peekDraft()).toBeNull();
  });

  it('a new provider draft belongs to its template', () => {
    keepDraft({ key: NEW_DRAFT, templateId: 'ollama', form: form({ name: 'GPU box' }), saved: form() });
    expect(draftFor(NEW_DRAFT, 'openai')).toBeNull();
    expect(draftFor(NEW_DRAFT, 'ollama')?.form.name).toBe('GPU box');
    dropDraft();
    expect(draftFor(NEW_DRAFT, 'ollama')).toBeNull();
  });
});
