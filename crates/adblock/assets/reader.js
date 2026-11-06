// BrowserMCP reader mode driver. Evaluated in an isolated world right after Readability.js,
// as `(<this function>)(apply)`. Returns the article as JSON, or null when the page has none.
// With apply=true it also replaces the page with a clean, readable version.
(function (apply) {
  // Readability modifies the document it parses: give it a copy.
  var article = new Readability(document.cloneNode(true), { charThreshold: 300 }).parse();
  if (!article) return null;

  var text = (article.textContent || "")
    .replace(/[ \t ]+/g, " ")
    .replace(/ ?\n[\s]*/g, "\n")
    .trim();

  if (apply) {
    var css = [
      ":root{color-scheme:light dark;--bg:#fbfaf7;--fg:#1f1f1f;--muted:#6a6a6a;--link:#0b57d0;--rule:#e3e1dc}",
      "@media (prefers-color-scheme:dark){:root{--bg:#17181a;--fg:#e6e4df;--muted:#a3a19c;--link:#8ab4f8;--rule:#2e2f33}}",
      "html{background:var(--bg)}",
      "body{margin:0;background:var(--bg);color:var(--fg);font:19px/1.65 Charter,'Iowan Old Style',Georgia,serif}",
      "main{max-width:40rem;margin:0 auto;padding:3rem 1.25rem 6rem}",
      "h1{font-size:2.1rem;line-height:1.2;margin:0 0 .5rem}",
      ".meta{color:var(--muted);font:15px/1.4 system-ui,sans-serif;margin:0 0 2rem;padding-bottom:1rem;border-bottom:1px solid var(--rule)}",
      "a{color:var(--link)}",
      "img,video,svg,iframe,figure{max-width:100%;height:auto}",
      "figure{margin:1.5rem 0}figcaption{color:var(--muted);font-size:.85em}",
      "pre,code{font:15px/1.5 ui-monospace,Consolas,monospace}pre{overflow:auto;padding:1rem;border:1px solid var(--rule)}",
      "blockquote{margin:1rem 0;padding-left:1rem;border-left:3px solid var(--rule);color:var(--muted)}",
      "table{border-collapse:collapse;display:block;overflow:auto}td,th{border:1px solid var(--rule);padding:.3rem .5rem}"
    ].join("\n");

    var doc = document.implementation.createHTMLDocument(article.title || document.title);
    if (article.lang) doc.documentElement.lang = article.lang;
    var meta = doc.createElement("meta");
    meta.name = "viewport";
    meta.content = "width=device-width, initial-scale=1";
    doc.head.appendChild(meta);
    var style = doc.createElement("style");
    style.textContent = css;
    doc.head.appendChild(style);

    var main = doc.createElement("main");
    var h1 = doc.createElement("h1");
    h1.textContent = article.title || document.title;
    main.appendChild(h1);
    var by = [article.byline, article.siteName].filter(Boolean).join(" · ");
    if (by) {
      var p = doc.createElement("p");
      p.className = "meta";
      p.textContent = by;
      main.appendChild(p);
    }
    var body = doc.createElement("article");
    body.innerHTML = article.content || "";
    // Nothing from the original page may run in the clean version.
    body.querySelectorAll("script,noscript,object,embed").forEach(function (el) { el.remove(); });
    body.querySelectorAll("*").forEach(function (el) {
      for (var i = el.attributes.length - 1; i >= 0; i--) {
        var name = el.attributes[i].name;
        if (/^on/i.test(name) || name === "style") el.removeAttribute(name);
      }
    });
    main.appendChild(body);
    doc.body.appendChild(main);

    document.replaceChild(document.importNode(doc.documentElement, true), document.documentElement);
    // Constructable sheets are not blocked by a strict page CSP the way <style> can be; this
    // also drops the page's own adopted sheets.
    try {
      var sheet = new CSSStyleSheet();
      sheet.replaceSync(css);
      document.adoptedStyleSheets = [sheet];
    } catch (e) {}
    window.scrollTo(0, 0);
  }

  return {
    title: article.title || document.title || "",
    byline: article.byline || null,
    siteName: article.siteName || null,
    excerpt: article.excerpt || null,
    lang: article.lang || null,
    text: text,
    applied: !!apply
  };
})
