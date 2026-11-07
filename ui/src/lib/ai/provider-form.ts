// The provider settings form (AiSettings.svelte) as plain data: filled from a provider or a
// template, compared with what is saved (unsaved changes), checked, and turned into the
// arguments of ai.provider_set / ai.provider_add. Kept free of Svelte so it is unit-tested.
// Validation mirrors crates/ai/src/config.rs so most mistakes show before a round trip; the
// core still has the last word.

import type { AiProvider, AiProviderSet } from '../api';
import type { AiApi, AiProviderAdd } from '../api/types-ai';
import { BLANK_TEMPLATE } from '../providers';

export interface HeaderRow {
  name: string;
  value: string;
}

export interface ProviderForm {
  /** User providers only (built-in names are fixed). */
  name: string;
  /** User providers only. */
  api: AiApi;
  baseUrl: string;
  /** A newly typed key; '' keeps the stored one. */
  apiKey: string;
  model: string;
  /** CLI providers only. */
  path: string;
  headers: HeaderRow[];
}

/** i18n keys of what is wrong, per field. */
export type FormErrors = Partial<Record<'name' | 'baseUrl' | 'headers', string>>;

/** Longest provider name the core accepts. */
export const MAX_NAME = 64;

/** A provider the user added (it can be renamed and removed). */
export function isUser(p: AiProvider): boolean {
  return p.builtin === false;
}

export function asApi(protocol: string): AiApi {
  return protocol === 'anthropic' || protocol === 'gemini' ? protocol : 'openai';
}

export function headerRows(headers: Record<string, string> | undefined): HeaderRow[] {
  return Object.entries(headers ?? {}).map(([name, value]) => ({ name, value }));
}

/** Rows as the core stores them: trimmed, rows without a name dropped. */
export function headerMap(rows: HeaderRow[]): Record<string, string> {
  const out: Record<string, string> = {};
  for (const row of rows) {
    const name = row.name.trim();
    if (name) out[name] = row.value.trim();
  }
  return out;
}

function sameHeaders(a: Record<string, string>, b: Record<string, string>): boolean {
  const ka = Object.keys(a);
  return ka.length === Object.keys(b).length && ka.every((k) => k in b && a[k] === b[k]);
}

export function formFrom(p: AiProvider): ProviderForm {
  return {
    name: p.name,
    api: asApi(p.protocol),
    baseUrl: p.base_url,
    apiKey: '',
    model: p.model ?? '',
    path: p.path ?? '',
    headers: headerRows(p.headers),
  };
}

const sameName = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase();

/** `base`, or `base 2`, `base 3`… whichever no provider uses yet. */
export function freeName(base: string, taken: readonly string[]): string {
  for (let n = 1; ; n++) {
    const name = n === 1 ? base : `${base} ${n}`;
    if (!taken.some((t) => sameName(t, name))) return name;
  }
}

/**
 * The form of a new provider. `template` is a built-in API provider to copy (its URL and API
 * type; its models are suggested until the first test), or null for a blank endpoint.
 */
export function formFromTemplate(template: AiProvider | null, taken: readonly string[]): ProviderForm {
  return {
    name: template ? freeName(template.name, taken) : '',
    api: template ? asApi(template.protocol) : 'openai',
    baseUrl: template?.base_url ?? '',
    apiKey: '',
    model: '',
    path: '',
    headers: [],
  };
}

/**
 * Only the fields the user changed: `form` against `saved` (the form as `p` was last loaded;
 * "" clears a field in the core). `p` says which fields the provider has at all.
 */
export function changes(form: ProviderForm, saved: ProviderForm, p: AiProvider): AiProviderSet {
  const args: AiProviderSet = { id: p.id };
  if (form.apiKey.trim()) args.api_key = form.apiKey.trim();
  if (form.model.trim() !== saved.model) args.model = form.model.trim();
  if (p.kind === 'cli' && form.path.trim() !== saved.path) args.path = form.path.trim();
  if (p.kind === 'api') {
    if (form.baseUrl.trim() !== saved.baseUrl) args.base_url = form.baseUrl.trim();
    const headers = headerMap(form.headers);
    if (!sameHeaders(headers, headerMap(saved.headers))) args.headers = headers;
  }
  if (isUser(p)) {
    if (form.name.trim() !== saved.name) args.name = form.name.trim();
    if (form.api !== saved.api) args.api = form.api;
  }
  return args;
}

/** Whether the form holds changes that are not saved yet. */
export function isDirty(form: ProviderForm, saved: ProviderForm, p: AiProvider): boolean {
  return Object.keys(changes(form, saved, p)).length > 1;
}

/**
 * The provider changed underneath an open form (a test loaded models, another window saved):
 * fields the user did not touch follow `fresh`, edited ones are kept.
 */
export function rebase(form: ProviderForm, saved: ProviderForm, fresh: ProviderForm): ProviderForm {
  const pick = <K extends keyof ProviderForm>(key: K): ProviderForm[K] => (JSON.stringify(form[key]) === JSON.stringify(saved[key]) ? fresh[key] : form[key]);
  return {
    name: pick('name'),
    api: pick('api'),
    baseUrl: pick('baseUrl'),
    apiKey: form.apiKey,
    model: pick('model'),
    path: pick('path'),
    // Plain copies: `form` may be a Svelte state proxy.
    headers: pick('headers').map((h) => ({ name: h.name, value: h.value })),
  };
}

/** Whether a new provider's form was touched (closing it would lose something). */
export function isDraftDirty(form: ProviderForm, start: ProviderForm): boolean {
  return (
    form.name !== start.name ||
    form.api !== start.api ||
    form.baseUrl !== start.baseUrl ||
    form.apiKey !== '' ||
    form.model !== '' ||
    form.headers.some((h) => h.name.trim() || h.value.trim())
  );
}

/** `ai.provider_add` arguments of a new provider's form. */
export function addArgs(form: ProviderForm, template: string | null): AiProviderAdd {
  const args: AiProviderAdd = {
    name: form.name.trim(),
    template: template ?? BLANK_TEMPLATE,
    api: form.api,
    base_url: form.baseUrl.trim(),
  };
  if (form.apiKey.trim()) args.api_key = form.apiKey.trim();
  if (form.model.trim()) args.model = form.model.trim();
  const headers = headerMap(form.headers);
  if (Object.keys(headers).length) args.headers = headers;
  return args;
}

/** An absolute http(s) URL with a host, as the core accepts for a base URL. */
export function isHttpUrl(text: string): boolean {
  try {
    const url = new URL(text);
    return (url.protocol === 'http:' || url.protocol === 'https:') && url.hostname !== '';
  } catch {
    return false;
  }
}

/** RFC 9110 token: what an HTTP header name may contain. */
const HEADER_NAME = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;

export interface ValidateOptions {
  kind: 'api' | 'cli';
  /** A user provider (or a new one): its name is editable and a URL is required. */
  user: boolean;
  /** Names of the other providers. */
  taken: readonly string[];
}

export function validate(form: ProviderForm, opts: ValidateOptions): FormErrors {
  const errors: FormErrors = {};
  if (opts.user) {
    const name = form.name.trim();
    if (!name) errors.name = 'aiProviders.error.nameEmpty';
    else if (name.length > MAX_NAME) errors.name = 'aiProviders.error.nameLong';
    else if (opts.taken.some((t) => sameName(t, name))) errors.name = 'aiProviders.error.nameTaken';
  }
  if (opts.kind === 'api') {
    const url = form.baseUrl.trim();
    // An empty URL resets a built-in to its preset; a user provider needs one.
    if (!url && opts.user) errors.baseUrl = 'aiProviders.error.urlEmpty';
    else if (url && !isHttpUrl(url)) errors.baseUrl = 'aiProviders.error.urlBad';
    const bad = form.headers.some((h) => (h.name.trim() ? !HEADER_NAME.test(h.name.trim()) || /[\r\n]/.test(h.value) : h.value.trim() !== ''));
    if (bad) errors.headers = 'aiProviders.error.header';
  }
  return errors;
}
