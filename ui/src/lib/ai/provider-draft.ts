// Unsaved edits of the open provider form, kept outside the components. Settings remounts its
// section when another one is opened and the AI panel drops its settings view when it closes;
// neither asks the form first, so a typed key, URL or header would be lost. The form leaves its
// draft here while it is dirty and takes it back when it opens again. One slot is enough: only
// one form is open at a time, and leaving a dirty form in the list itself asks first.
//
// Plain module state, not persisted: a reload or a closed window still loses the draft (the
// form asks before that with `beforeunload`).

import type { ProviderForm } from './provider-form';

/** `key` of the "add provider" form (provider ids never contain NUL). */
export const NEW_DRAFT = '\u0000new';

export interface ProviderDraft {
  /** The provider id, or NEW_DRAFT. */
  key: string;
  /** Add form: the template it was started from. */
  templateId?: string;
  form: ProviderForm;
  /** What the edits were made against: the provider as loaded, or the add form's start. */
  saved: ProviderForm;
}

let draft: ProviderDraft | null = null;

export function keepDraft(next: ProviderDraft): void {
  draft = structuredClone(next);
}

/** The kept draft, if any (a copy). */
export function peekDraft(): ProviderDraft | null {
  return draft && structuredClone(draft);
}

/** The draft of the form `key` (and, for the add form, `templateId`), if one is kept. */
export function draftFor(key: string, templateId?: string): ProviderDraft | null {
  if (!draft || draft.key !== key) return null;
  if (key === NEW_DRAFT && draft.templateId !== templateId) return null;
  return structuredClone(draft);
}

/** Forget the draft of `key` (any draft without one): it was saved or discarded. */
export function dropDraft(key?: string): void {
  if (key === undefined || draft?.key === key) draft = null;
}
