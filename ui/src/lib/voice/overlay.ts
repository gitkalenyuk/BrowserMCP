// Shows the interim (not yet final) dictation dimmed inside a textarea / input: a mirror of
// the field's text is laid over it, with the interim part faded, while the field's own text
// is made transparent (its caret stays visible). Any failure falls back to fading the whole
// field, which keeps the text readable.

const COPIED = [
  'font-family',
  'font-size',
  'font-weight',
  'font-style',
  'line-height',
  'letter-spacing',
  'word-spacing',
  'text-align',
  'text-indent',
  'text-transform',
  'padding-top',
  'padding-right',
  'padding-bottom',
  'padding-left',
  'border-top-width',
  'border-right-width',
  'border-bottom-width',
  'border-left-width',
  'box-sizing',
  'tab-size',
] as const;

export class InterimOverlay {
  #target: HTMLTextAreaElement | HTMLInputElement;
  #mirror: HTMLDivElement | null = null;
  #saved: { color: string; caret: string; opacity: string } | null = null;
  #onScroll = () => this.#place();

  constructor(target: HTMLTextAreaElement | HTMLInputElement) {
    this.#target = target;
  }

  /** Show `value` with `[start, start + length)` faded. */
  show(start: number, length: number): void {
    const t = this.#target;
    if (length <= 0) return this.hide();
    try {
      if (!this.#saved) {
        // getComputedStyle is live: read the colors before the field's own text goes away.
        const css = getComputedStyle(t);
        const color = css.color;
        const caret = css.caretColor && css.caretColor !== 'auto' ? css.caretColor : color;
        this.#saved = { color: t.style.color, caret: t.style.caretColor, opacity: t.style.opacity };
        t.style.caretColor = caret;
        t.style.color = 'transparent';
        const m = document.createElement('div');
        m.className = 'bmcp-voice-mirror';
        m.setAttribute('aria-hidden', 'true');
        for (const p of COPIED) m.style.setProperty(p, css.getPropertyValue(p));
        Object.assign(m.style, {
          position: 'fixed',
          pointerEvents: 'none',
          overflow: 'hidden',
          whiteSpace: t instanceof HTMLTextAreaElement ? 'pre-wrap' : 'pre',
          overflowWrap: 'break-word',
          borderStyle: 'solid',
          borderColor: 'transparent',
          background: 'transparent',
          color,
          zIndex: '2147483000',
          margin: '0',
        });
        document.body.appendChild(m);
        this.#mirror = m;
        window.addEventListener('scroll', this.#onScroll, true);
        window.addEventListener('resize', this.#onScroll);
        t.addEventListener('scroll', this.#onScroll);
      }
      const m = this.#mirror!;
      const v = t.value;
      m.replaceChildren(
        document.createTextNode(v.slice(0, start)),
        Object.assign(document.createElement('span'), { textContent: v.slice(start, start + length), style: 'opacity:0.5' }),
        // A trailing newline needs a character after it to take up a line.
        document.createTextNode(v.slice(start + length) + '​'),
      );
      this.#place();
    } catch {
      this.hide();
      t.style.opacity = '0.6';
      this.#saved ??= { color: '', caret: '', opacity: '' };
    }
  }

  #place(): void {
    const m = this.#mirror;
    if (!m) return;
    const r = this.#target.getBoundingClientRect();
    Object.assign(m.style, { left: `${r.left}px`, top: `${r.top}px`, width: `${r.width}px`, height: `${r.height}px` });
    m.scrollTop = this.#target.scrollTop;
    m.scrollLeft = this.#target.scrollLeft;
  }

  hide(): void {
    const t = this.#target;
    if (this.#saved) {
      t.style.color = this.#saved.color;
      t.style.caretColor = this.#saved.caret;
      t.style.opacity = this.#saved.opacity;
      this.#saved = null;
    }
    if (this.#mirror) {
      this.#mirror.remove();
      this.#mirror = null;
      window.removeEventListener('scroll', this.#onScroll, true);
      window.removeEventListener('resize', this.#onScroll);
      t.removeEventListener('scroll', this.#onScroll);
    }
  }
}
