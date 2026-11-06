// Find in page (Ctrl+F), run by crates/pagetools/src/find.rs in an isolated world.
//
// find(query, caseSensitive, wanted): mark every visible match with the CSS Custom
// Highlight API (no DOM changes besides one <style>), mark match number `wanted` (wraps
// around, may be negative) as the current one and scroll it into view.
// Returns {count, index, context}. An empty query clears everything.
(function (query, caseSensitive, wanted) {
  const ALL = 'bmcp-find';
  const CURRENT = 'bmcp-find-current';
  const STYLE_ID = 'bmcp-find-style';
  const MAX = 1000;

  const clear = () => {
    CSS.highlights.delete(ALL);
    CSS.highlights.delete(CURRENT);
    const style = document.getElementById(STYLE_ID);
    if (style) style.remove();
  };
  if (!window.CSS || !CSS.highlights) return { count: 0, index: 0, context: '', unsupported: true };
  if (!query) {
    clear();
    return { count: 0, index: 0, context: '' };
  }

  if (!document.getElementById(STYLE_ID)) {
    const style = document.createElement('style');
    style.id = STYLE_ID;
    style.textContent =
      '::highlight(bmcp-find){background-color:#fde047;color:#000}' +
      '::highlight(bmcp-find-current){background-color:#f97316;color:#000}';
    (document.head || document.documentElement).appendChild(style);
  }

  const needle = caseSensitive ? query : query.toLowerCase();
  const skip = 'script,style,noscript,template,textarea,select';
  const walker = document.createTreeWalker(document.body || document.documentElement, NodeFilter.SHOW_TEXT, {
    acceptNode(node) {
      const parent = node.parentElement;
      return parent && !parent.closest(skip) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT;
    },
  });
  const ranges = [];
  for (let node = walker.nextNode(); node && ranges.length < MAX; node = walker.nextNode()) {
    let text = node.data;
    // Lower-casing may change the length of some letters (e.g. "İ"); then offsets would
    // be wrong, so such text nodes are searched case-sensitively.
    if (!caseSensitive && text.toLowerCase().length === text.length) text = text.toLowerCase();
    for (let at = text.indexOf(needle); at !== -1 && ranges.length < MAX; at = text.indexOf(needle, at + needle.length)) {
      const range = new Range();
      range.setStart(node, at);
      range.setEnd(node, at + needle.length);
      // Hidden text (display:none, collapsed) has no boxes: not a match a human could see.
      if (range.getClientRects().length > 0) ranges.push(range);
    }
  }
  CSS.highlights.set(ALL, new Highlight(...ranges));
  if (!ranges.length) {
    CSS.highlights.delete(CURRENT);
    return { count: 0, index: 0, context: '' };
  }

  const count = ranges.length;
  const index = ((wanted % count) + count) % count;
  const current = ranges[index];
  CSS.highlights.set(CURRENT, new Highlight(current));

  // Scroll: first the nearest scrollable parents, then the window, so the match ends up
  // in the middle of the viewport.
  const element = current.startContainer.parentElement;
  if (element) element.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  const rect = current.getBoundingClientRect();
  if (rect.top < 0 || rect.bottom > innerHeight || rect.left < 0 || rect.right > innerWidth) {
    window.scrollBy({
      top: rect.top - innerHeight / 2,
      left: rect.left < 0 || rect.right > innerWidth ? rect.left - innerWidth / 2 : 0,
      behavior: 'instant',
    });
  }

  const data = current.startContainer.data;
  const start = current.startOffset;
  const context = data
    .slice(Math.max(0, start - 40), current.endOffset + 40)
    .replace(/\s+/g, ' ')
    .trim();
  return { count, index, context };
})
