// Monaco for the Scripts panel. LAZY: only imported with `import('./monaco')` when the panel
// shows an editor, so Vite puts Monaco (several MB) in its own chunks and the chrome bundle
// stays small.
//
// IntelliSense: the generated script typings (crates/script/types/browser.d.ts, written by
// `cargo xtask gen-sdk`) are imported as text at BUILD time (`?raw`), so every UI build ships
// the typings of the commands of that same checkout.

import * as monaco from 'monaco-editor';
import EditorWorker from 'monaco-editor/editor/editor.worker?worker';
import TsWorker from 'monaco-editor/language/typescript/ts.worker?worker';
import browserTypes from '../../../../crates/script/types/browser.d.ts?raw';
import { t } from '../i18n/index.svelte';

// Workers: once MonacoEnvironment.getWorker exists Monaco asks it for EVERY worker, so the
// TypeScript language service must get its own (Vite bundles each `?worker` import).
self.MonacoEnvironment = {
  getWorker: (_id: string, label: string) =>
    label === 'typescript' || label === 'javascript' ? new TsWorker() : new EditorWorker(),
};

const ts = monaco.typescript;
ts.typescriptDefaults.setCompilerOptions({
  target: ts.ScriptTarget.ES2020,
  module: ts.ModuleKind.ESNext,
  // Every script is a module, so top-level `await` type-checks (TS ModuleDetectionKind.Force).
  moduleDetection: 3,
  lib: ['es2022'],
  strict: true,
  noEmit: true,
  allowNonTsExtensions: true,
});
ts.typescriptDefaults.setDiagnosticsOptions({
  noSemanticValidation: false,
  noSyntaxValidation: false,
  // 6053: `/// <reference path="…/browser.d.ts" />` in example scripts; the typings are loaded anyway.
  diagnosticCodesToIgnore: [6053],
});
ts.typescriptDefaults.addExtraLib(browserTypes, 'file:///bmcp/browser.d.ts');

/** CSS custom property as #rrggbb (Monaco themes need hex colors). */
function cssColor(name: string, fallback: string): string {
  const probe = document.createElement('span');
  probe.style.color = `var(${name}, ${fallback})`;
  document.body.appendChild(probe);
  const rgb = getComputedStyle(probe).color;
  probe.remove();
  const m = rgb.match(/\d+(\.\d+)?/g);
  if (!m || m.length < 3) return fallback;
  return '#' + m.slice(0, 3).map((n) => Math.round(Number(n)).toString(16).padStart(2, '0')).join('');
}

/** Monaco themes from the chrome's tokens (call again after the theme changes). */
export function defineThemes(): void {
  const colors = (fallbackBg: string) => ({
    'editor.background': cssColor('--surface', fallbackBg),
    'editorGutter.background': cssColor('--surface', fallbackBg),
  });
  monaco.editor.defineTheme('bmcp-dark', { base: 'vs-dark', inherit: true, rules: [], colors: colors('#24272d') });
  monaco.editor.defineTheme('bmcp-light', { base: 'vs', inherit: true, rules: [], colors: colors('#ffffff') });
}

export type EditorKey = 'save' | 'run' | 'continue' | 'stop' | 'stepOver' | 'stepInto' | 'stepOut' | 'toggleBreakpoint';

export interface ScriptEditorOptions {
  value: string;
  /** Model path, e.g. "login" -> file:///scripts/login.ts. */
  path: string;
  theme: 'dark' | 'light';
  onChange: (value: string) => void;
  /** Breakpoint lines after a gutter click or an edit that moved them. */
  onBreakpoints: (lines: number[]) => void;
  onKey: (key: EditorKey) => void;
}

/** A thin controller over one Monaco editor: content, breakpoints, current/error line. */
export class ScriptEditor {
  readonly editor: monaco.editor.IStandaloneCodeEditor;
  #opts: ScriptEditorOptions;
  #breakpoints: monaco.editor.IEditorDecorationsCollection;
  #current: monaco.editor.IEditorDecorationsCollection;
  #silent = false;

  constructor(el: HTMLElement, opts: ScriptEditorOptions) {
    this.#opts = opts;
    defineThemes();
    this.editor = monaco.editor.create(el, {
      model: modelFor(opts.path, opts.value),
      theme: opts.theme === 'dark' ? 'bmcp-dark' : 'bmcp-light',
      automaticLayout: true,
      glyphMargin: true,
      minimap: { enabled: false },
      fontSize: 13,
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--font-mono') || undefined,
      scrollBeyondLastLine: false,
      tabSize: 2,
      fixedOverflowWidgets: true,
      renderLineHighlight: 'line',
    });
    this.#breakpoints = this.editor.createDecorationsCollection();
    this.#current = this.editor.createDecorationsCollection();

    this.editor.onDidChangeModelContent(() => {
      if (this.#silent) return;
      opts.onChange(this.editor.getValue());
      // Breakpoint decorations move with their code; report where they are now.
      opts.onBreakpoints(this.breakpointLines());
    });
    this.editor.onMouseDown((e) => {
      const target = e.target.type;
      const gutter = target === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN || target === monaco.editor.MouseTargetType.GUTTER_LINE_NUMBERS;
      if (gutter && e.target.position) this.#toggle(e.target.position.lineNumber);
    });

    // Keys (the chrome lets these through for elements with data-own-keys: EDITOR_KEYS in logic.ts).
    const K = monaco.KeyCode;
    const M = monaco.KeyMod;
    const bind: Array<[number, EditorKey]> = [
      [M.CtrlCmd | K.KeyS, 'save'],
      [K.F5, 'continue'],
      [M.Shift | K.F5, 'stop'],
      [K.F9, 'toggleBreakpoint'],
      [K.F10, 'stepOver'],
      [K.F11, 'stepInto'],
      [M.Shift | K.F11, 'stepOut'],
    ];
    for (const [keys, key] of bind) {
      this.editor.addCommand(keys, () => (key === 'toggleBreakpoint' ? this.#toggleAtCursor() : opts.onKey(key)));
    }
  }

  /** Replace the text (and model) without firing onChange, e.g. when another script opens. */
  open(path: string, value: string): void {
    this.#silent = true;
    try {
      const model = modelFor(path, value);
      if (this.editor.getModel() !== model) this.editor.setModel(model);
      if (model.getValue() !== value) model.setValue(value);
    } finally {
      this.#silent = false;
    }
  }

  setTheme(theme: 'dark' | 'light'): void {
    defineThemes();
    monaco.editor.setTheme(theme === 'dark' ? 'bmcp-dark' : 'bmcp-light');
  }

  setBreakpoints(lines: number[], verified: number[] | null): void {
    this.#breakpoints.set(
      lines.map((line) => ({
        range: new monaco.Range(line, 1, line, 1),
        options: {
          isWholeLine: true,
          stickiness: monaco.editor.TrackedRangeStickiness.NeverGrowsWhenTypingAtEdges,
          glyphMarginClassName: verified && !verified.includes(line) ? 'bmcp-bp bmcp-bp-unverified' : 'bmcp-bp',
          glyphMarginHoverMessage: { value: t('scripts.breakpointHover') },
        },
      })),
    );
  }

  breakpointLines(): number[] {
    const lines = this.#breakpoints.getRanges().map((r) => r.startLineNumber);
    return [...new Set(lines)].sort((a, b) => a - b);
  }

  /** Highlight the paused line (kind "current") or an error line; null clears. */
  setMarkedLine(line: number | null, kind: 'current' | 'error' = 'current'): void {
    if (line === null) {
      this.#current.clear();
      return;
    }
    this.#current.set([
      {
        range: new monaco.Range(line, 1, line, 1),
        options: {
          isWholeLine: true,
          className: kind === 'current' ? 'bmcp-current-line' : 'bmcp-error-line',
          glyphMarginClassName: kind === 'current' ? 'bmcp-current-arrow' : undefined,
        },
      },
    ]);
    this.revealLine(line);
  }

  revealLine(line: number): void {
    this.editor.revealLineInCenterIfOutsideViewport(line);
    this.editor.setPosition({ lineNumber: line, column: 1 });
  }

  focus(): void {
    this.editor.focus();
  }

  dispose(): void {
    this.editor.dispose();
  }

  #toggle(line: number): void {
    const lines = this.breakpointLines();
    const next = lines.includes(line) ? lines.filter((l) => l !== line) : [...lines, line].sort((a, b) => a - b);
    this.#opts.onBreakpoints(next);
  }

  #toggleAtCursor(): void {
    const pos = this.editor.getPosition();
    if (pos) this.#toggle(pos.lineNumber);
  }
}

/** One model per script path (keeps undo history per script while the chrome runs). */
function modelFor(path: string, value: string): monaco.editor.ITextModel {
  const uri = monaco.Uri.parse(`file:///scripts/${encodeURIComponent(path)}.ts`);
  return monaco.editor.getModel(uri) ?? monaco.editor.createModel(value, 'typescript', uri);
}
