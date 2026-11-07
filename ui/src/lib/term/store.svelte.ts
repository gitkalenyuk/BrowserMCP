import type { BrowserApi } from '../api/client';
import type { TermInfo } from '../api/types';
import { t } from '../i18n/index.svelte';

export interface TermTab {
  id: string;
  /** Program that runs, e.g. `pwsh.exe`. */
  shell: string;
  /** Short label for the tab strip. */
  label: string;
  exitCode: number | null;
  exited: boolean;
}

/** Output kept per terminal so a terminal re-attached after the panel was closed can replay it. */
const BUFFER_MAX = 512 * 1024;

/**
 * Cursor position queries (`ESC[6n`) in replayed output. Live, xterm.js answers them (the shell
 * asked); on replay the shell is not waiting, so an answer would arrive as stray input. (The
 * core already answers ConPTY's very first query and drops it from the output.)
 */
const CPR_QUERY = /\x1b\[\??6n/g;

export function shellLabel(shell: string): string {
  const base = shell.split(/[\\/]/).pop() ?? shell;
  return base.replace(/\.exe$/i, '');
}

export function replayText(chunks: string[]): string {
  return chunks.join('').replace(CPR_QUERY, '');
}

/**
 * PTY terminals of the core (`term.*`). Output is buffered here from the moment the chrome
 * starts — also for terminals opened by someone else (REST, an agent) — and handed to the
 * xterm.js view that is attached to a terminal.
 */
export class TerminalStore {
  readonly api: BrowserApi;
  tabs = $state<TermTab[]>([]);
  activeId = $state<string | null>(null);
  error = $state<string | null>(null);
  /** Shells that failed to start in this session (e.g. bash without Git for Windows). */
  unavailable = $state<string[]>([]);
  /** The panel started its first terminal by itself already (once per chrome session). */
  autoOpened = false;
  #buffers = new Map<string, { chunks: string[]; size: number }>();
  #sinks = new Map<string, (data: string) => void>();
  #unsubs: Array<() => void> = [];
  #refreshTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(api: BrowserApi) {
    this.api = api;
  }

  start(): void {
    this.#unsubs.push(
      this.api.on('term.output', ({ id, data }) => {
        this.#append(id, data);
        this.#sinks.get(id)?.(data);
        if (!this.tabs.some((t) => t.id === id)) this.#refreshSoon();
      }),
      this.api.on('term.exit', ({ id, code }) => {
        const t = this.tabs.find((x) => x.id === id);
        if (t) {
          t.exited = true;
          t.exitCode = code;
        }
        const note = `\r\n\x1b[2m[process exited${code === null ? '' : ` with code ${code}`}]\x1b[0m\r\n`;
        this.#append(id, note);
        this.#sinks.get(id)?.(note);
      }),
      this.api.onStatus((s) => {
        if (s === 'open') void this.refresh();
      }),
    );
  }

  stop(): void {
    this.#unsubs.forEach((u) => u());
    this.#unsubs = [];
  }

  /** Adopt terminals the core has but this view does not know yet (`term.list`). */
  async refresh(): Promise<void> {
    let terminals: TermInfo[];
    try {
      ({ terminals } = await this.api.call('term.list'));
    } catch {
      return;
    }
    for (const info of terminals) this.#adopt(info);
    if (!this.activeId && this.tabs.length) this.activeId = this.tabs[this.tabs.length - 1].id;
  }

  async open(shell: string, cols = 80, rows = 24): Promise<string | null> {
    this.error = null;
    try {
      const info = await this.api.call('term.open', { shell, cols, rows });
      this.#adopt(info);
      this.activeId = info.id;
      this.unavailable = this.unavailable.filter((s) => s !== shell);
      return info.id;
    } catch (err) {
      this.error = t('term.cannotStart', { shell, error: (err as Error).message });
      if (!this.unavailable.includes(shell)) this.unavailable = [...this.unavailable, shell];
      return null;
    }
  }

  async close(id: string): Promise<void> {
    const i = this.tabs.findIndex((t) => t.id === id);
    if (i < 0) return;
    const wasRunning = !this.tabs[i].exited;
    this.tabs = this.tabs.filter((t) => t.id !== id);
    this.#buffers.delete(id);
    this.#sinks.delete(id);
    if (this.activeId === id) this.activeId = this.tabs[Math.min(i, this.tabs.length - 1)]?.id ?? null;
    if (wasRunning) {
      try {
        await this.api.call('term.close', { id });
      } catch {
        // Already gone.
      }
    }
  }

  write(id: string, data: string): void {
    const t = this.tabs.find((x) => x.id === id);
    if (!t || t.exited) return;
    this.api.call('term.write', { id, data }).catch((err: Error) => (this.error = err.message));
  }

  resize(id: string, cols: number, rows: number): void {
    const t = this.tabs.find((x) => x.id === id);
    if (!t || t.exited) return;
    this.api.call('term.resize', { id, cols, rows }).catch(() => {});
  }

  /** Connect a view: returns what was printed so far and streams the rest to `sink`. */
  attach(id: string, sink: (data: string) => void): string {
    this.#sinks.set(id, sink);
    return replayText(this.#buffers.get(id)?.chunks ?? []);
  }

  detach(id: string, sink: (data: string) => void): void {
    if (this.#sinks.get(id) === sink) this.#sinks.delete(id);
  }

  #adopt(info: TermInfo): void {
    if (this.tabs.some((t) => t.id === info.id)) return;
    const exited = info.exit_code !== null;
    this.tabs = [...this.tabs, { id: info.id, shell: info.shell, label: shellLabel(info.shell), exitCode: info.exit_code, exited }];
  }

  #append(id: string, data: string): void {
    let b = this.#buffers.get(id);
    if (!b) this.#buffers.set(id, (b = { chunks: [], size: 0 }));
    b.chunks.push(data);
    b.size += data.length;
    while (b.size > BUFFER_MAX && b.chunks.length > 1) b.size -= b.chunks.shift()!.length;
  }

  #refreshSoon(): void {
    if (this.#refreshTimer) return;
    this.#refreshTimer = setTimeout(() => {
      this.#refreshTimer = null;
      void this.refresh();
    }, 100);
  }
}
