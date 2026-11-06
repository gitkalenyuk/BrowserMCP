import type { BrowserApi } from '../api/client';
import type {
  DebugAction,
  DebugEnded,
  DebugOutput,
  DebugPaused,
  ScriptInfo,
} from '../api/types-scripts';
import { debugEndEntry, endEntry, inferInputs, newScriptTemplate, outcomeEntries, validName, type ConsoleEntry } from './logic';

export type DebugStatus = 'starting' | 'running' | 'paused';

export interface DebugView {
  /** null while script.debug_start is in flight. */
  id: string | null;
  status: DebugStatus;
  paused: DebugPaused | null;
}

const CONSOLE_MAX = 2000;

/**
 * State of the Scripts panel: the library (`script.list/get/save/delete`), the open script and
 * its unsaved edits, Run (`script.run`) and the step debugger (`script.debug_start/debug_cmd` +
 * `script.debug_*` events). One store per API connection, so closing and reopening the panel
 * keeps edits, breakpoints and the console.
 */
export class ScriptsStore {
  readonly api: BrowserApi;
  scripts = $state<ScriptInfo[]>([]);
  loaded = $state(false);
  /** Name of the open script (null = nothing open). */
  current = $state<string | null>(null);
  /** Editor text of the open script. */
  code = $state('');
  /** Text as last loaded / saved (dirty = code differs). */
  saved = $state('');
  readonly dirty = $derived(this.current !== null && this.code !== this.saved);
  readonly inputNames = $derived(inferInputs(this.code));
  /** Run form values, by input name (shared between scripts). */
  inputValues = $state<Record<string, string>>({});
  /** Breakpoint lines per script. */
  breakpoints = $state<Record<string, number[]>>({});
  /** Lines the debugger confirmed for the running session (others are drawn hollow). */
  verified = $state<number[] | null>(null);
  console = $state<ConsoleEntry[]>([]);
  running = $state(false);
  debug = $state<DebugView | null>(null);
  /** Line of the last error (highlighted until the next run). */
  errorLine = $state<number | null>(null);
  /** Last failed API call, shown above the editor. */
  error = $state<string | null>(null);
  /** Unsaved texts of scripts that are not open right now. */
  #drafts = new Map<string, { code: string; saved: string }>();
  /** script.debug_* events that arrived before debug_start answered with the id. */
  #early: Array<{ topic: string; payload: { id: string } }> = [];
  #unsubs: Array<() => void> = [];

  constructor(api: BrowserApi) {
    this.api = api;
  }

  get currentBreakpoints(): number[] {
    return this.current ? (this.breakpoints[this.current] ?? []) : [];
  }

  /** Subscribe to debugger events (idempotent). */
  start(): void {
    if (this.#unsubs.length) return;
    this.#unsubs.push(
      this.api.on('script.debug_paused', (e) => this.#onEvent('paused', e)),
      this.api.on('script.debug_resumed', (e) => this.#onEvent('resumed', e)),
      this.api.on('script.debug_output', (e) => this.#onEvent('output', e)),
      this.api.on('script.debug_ended', (e) => this.#onEvent('ended', e)),
    );
  }

  stop(): void {
    this.#unsubs.forEach((u) => u());
    this.#unsubs = [];
  }

  async #call<T>(what: string, fn: () => Promise<T>): Promise<T | undefined> {
    try {
      const value = await fn();
      this.error = null;
      return value;
    } catch (err) {
      this.error = `${what}: ${err instanceof Error ? err.message : String(err)}`;
      return undefined;
    }
  }

  // ---- library ----------------------------------------------------------------------------

  async refresh(): Promise<void> {
    const res = await this.#call('Listing scripts', () => this.api.call('script.list'));
    if (res) this.scripts = res.scripts;
    this.loaded = true;
    if (this.current === null && this.scripts.length > 0) await this.open(this.scripts[0].name);
  }

  async open(name: string): Promise<void> {
    if (name === this.current) return;
    this.#stash();
    const draft = this.#drafts.get(name);
    if (draft) {
      this.#show(name, draft.code, draft.saved);
      return;
    }
    const res = await this.#call(`Opening ${name}`, () => this.api.call('script.get', { name }));
    if (res) this.#show(name, res.code, res.code);
  }

  #show(name: string, code: string, saved: string): void {
    this.current = name;
    this.code = code;
    this.saved = saved;
    this.errorLine = null;
  }

  /** Keep the open script's unsaved text when another one opens. */
  #stash(): void {
    if (this.current !== null && this.code !== this.saved) {
      this.#drafts.set(this.current, { code: this.code, saved: this.saved });
    } else if (this.current !== null) {
      this.#drafts.delete(this.current);
    }
  }

  isDirty(name: string): boolean {
    return name === this.current ? this.dirty : this.#drafts.has(name);
  }

  async create(name: string): Promise<boolean> {
    if (!validName(name)) {
      this.error = `"${name}" is not a valid name: use letters, digits, - and _ (max 64)`;
      return false;
    }
    if (this.scripts.some((s) => s.name === name)) {
      this.error = `A script named "${name}" already exists`;
      return false;
    }
    const code = newScriptTemplate(name);
    const ok = await this.#call(`Creating ${name}`, () => this.api.call('script.save', { name, code }));
    if (!ok) return false;
    await this.refresh();
    await this.open(name);
    return true;
  }

  async rename(from: string, to: string): Promise<boolean> {
    if (from === to) return true;
    if (!validName(to)) {
      this.error = `"${to}" is not a valid name: use letters, digits, - and _ (max 64)`;
      return false;
    }
    if (this.scripts.some((s) => s.name === to)) {
      this.error = `A script named "${to}" already exists`;
      return false;
    }
    // Saved text goes to the new name; unsaved edits stay unsaved (as a draft of the new name).
    const saved =
      from === this.current
        ? this.saved
        : (this.#drafts.get(from)?.saved ?? (await this.api.call('script.get', { name: from }).catch(() => null))?.code);
    if (saved === undefined || saved === null) return false;
    const ok = await this.#call(`Renaming ${from}`, async () => {
      await this.api.call('script.save', { name: to, code: saved });
      await this.api.call('script.delete', { name: from });
      return true;
    });
    if (!ok) return false;
    const draft = this.#drafts.get(from);
    this.#drafts.delete(from);
    if (draft) this.#drafts.set(to, draft);
    if (this.breakpoints[from]) {
      const { [from]: lines, ...rest } = this.breakpoints;
      this.breakpoints = { ...rest, [to]: lines };
    }
    if (this.current === from) this.current = to;
    await this.refresh();
    return true;
  }

  async remove(name: string): Promise<void> {
    const ok = await this.#call(`Deleting ${name}`, () => this.api.call('script.delete', { name }));
    if (!ok) return;
    this.#drafts.delete(name);
    if (this.current === name) {
      this.current = null;
      this.code = '';
      this.saved = '';
    }
    await this.refresh();
  }

  async save(): Promise<void> {
    const name = this.current;
    if (name === null) return;
    const code = this.code;
    const ok = await this.#call(`Saving ${name}`, () => this.api.call('script.save', { name, code }));
    if (!ok) return;
    if (this.current === name) this.saved = code;
    this.#drafts.delete(name);
    const res = await this.#call('Listing scripts', () => this.api.call('script.list'));
    if (res) this.scripts = res.scripts;
  }

  // ---- run ----------------------------------------------------------------------------------

  /** Inputs of the open script with a value (empty fields are left out: `inputs.x` is undefined). */
  #inputs(): Record<string, string> {
    const out: Record<string, string> = {};
    for (const name of this.inputNames) {
      const v = this.inputValues[name];
      if (v !== undefined && v !== '') out[name] = v;
    }
    return out;
  }

  /** Saved script by name; the editor's text when it has unsaved edits (line numbers match). */
  #source(): { name: string } | { code: string } {
    return this.dirty || this.current === null ? { code: this.code } : { name: this.current };
  }

  #log(entry: ConsoleEntry): void {
    const next = [...this.console, entry];
    this.console = next.length > CONSOLE_MAX ? next.slice(next.length - CONSOLE_MAX) : next;
  }

  clearConsole(): void {
    this.console = [];
  }

  async run(): Promise<void> {
    if (this.running || this.debug || this.current === null) return;
    this.running = true;
    this.errorLine = null;
    this.console = [{ kind: 'info', text: `▶ ${this.current}${this.dirty ? ' (unsaved)' : ''}` }];
    try {
      const outcome = await this.api.call('script.run', { ...this.#source(), inputs: this.#inputs() });
      this.console = [...this.console, ...outcomeEntries(outcome)];
      this.errorLine = outcome.error?.line ?? null;
    } catch (err) {
      this.#log({ kind: 'error', text: err instanceof Error ? err.message : String(err) });
    } finally {
      this.running = false;
    }
  }

  // ---- debugger -----------------------------------------------------------------------------

  async debugStart(): Promise<void> {
    if (this.running || this.debug || this.current === null) return;
    this.debug = { id: null, status: 'starting', paused: null };
    this.errorLine = null;
    this.verified = null;
    this.#early = [];
    this.console = [{ kind: 'info', text: `🐞 ${this.current}${this.dirty ? ' (unsaved)' : ''}` }];
    try {
      const res = await this.api.call('script.debug_start', {
        ...this.#source(),
        inputs: this.#inputs(),
        breakpoints: this.currentBreakpoints,
      });
      if (res.error) {
        this.debug = null;
        this.#log(endEntry(false, 0, res.error, false));
        this.errorLine = res.error.line ?? null;
        return;
      }
      this.verified = res.breakpoints;
      if (this.debug) this.debug = { ...this.debug, id: res.id, status: 'running' };
      // Replay what arrived before we knew the id (e.g. an immediate breakpoint hit).
      const early = this.#early.filter((e) => e.payload.id === res.id);
      this.#early = [];
      for (const e of early) this.#apply(e.topic, e.payload);
    } catch (err) {
      this.debug = null;
      this.#log({ kind: 'error', text: err instanceof Error ? err.message : String(err) });
    }
  }

  async debugCmd(action: Exclude<DebugAction, 'set_breakpoints'>): Promise<void> {
    const id = this.debug?.id;
    if (!id) return;
    await this.#call('Debugger', () => this.api.call('script.debug_cmd', { id, action }));
  }

  /** F5: continue when debugging, else start the debugger. */
  async continueOrStart(): Promise<void> {
    if (this.debug) {
      if (this.debug.status === 'paused') await this.debugCmd('continue');
    } else {
      await this.debugStart();
    }
  }

  async setBreakpoints(lines: number[]): Promise<void> {
    const name = this.current;
    if (name === null) return;
    const same = JSON.stringify(lines) === JSON.stringify(this.breakpoints[name] ?? []);
    if (same) return;
    this.breakpoints = { ...this.breakpoints, [name]: lines };
    const id = this.debug?.id;
    if (!id) return;
    const res = await this.#call('Breakpoints', () =>
      this.api.call('script.debug_cmd', { id, action: 'set_breakpoints', lines }),
    );
    if (res?.breakpoints) this.verified = res.breakpoints;
  }

  #onEvent(topic: string, payload: { id: string }): void {
    const debug = this.debug;
    if (!debug) return;
    if (debug.id === null) {
      this.#early.push({ topic, payload });
      return;
    }
    if (payload.id === debug.id) this.#apply(topic, payload);
  }

  #apply(topic: string, payload: unknown): void {
    if (!this.debug) return;
    switch (topic) {
      case 'paused':
        this.debug = { ...this.debug, status: 'paused', paused: payload as DebugPaused };
        break;
      case 'resumed':
        this.debug = { ...this.debug, status: 'running', paused: null };
        break;
      case 'output': {
        const out = payload as DebugOutput;
        if (out.stream === 'result') {
          let text = out.text;
          try {
            text = JSON.stringify(JSON.parse(out.text), null, 2);
          } catch {
            // keep raw
          }
          this.#log({ kind: 'result', text });
        } else {
          this.#log({ kind: out.stream, text: out.text });
        }
        break;
      }
      case 'ended': {
        const ended = payload as DebugEnded;
        // The final "error: Uncaught …" stderr lines are summed up by the end entry.
        if (ended.error) {
          let cut = this.console.length - 1;
          while (cut >= 0 && !(this.console[cut].kind === 'stderr' && this.console[cut].text.startsWith('error: '))) cut--;
          if (cut >= 0) this.console = this.console.slice(0, cut);
        }
        this.#log(debugEndEntry(ended));
        this.errorLine = ended.error?.line ?? null;
        this.debug = null;
        this.verified = null;
        break;
      }
    }
  }
}

const stores = new WeakMap<BrowserApi, ScriptsStore>();

/** The panel's store for this API connection (created on first use). */
export function scriptsStore(api: BrowserApi): ScriptsStore {
  let store = stores.get(api);
  if (!store) {
    store = new ScriptsStore(api);
    stores.set(api, store);
  }
  return store;
}
