// Fixture server of the e2e suite: tests/fixtures/sites/ over HTTP, plus the small JSON API
// of net.html (same routes as tests/fixtures/sites/net_server.py, without the WebSocket) and
// the "ads" the adblock test expects to be blocked.
//
//   node tests/e2e/fixtures/server.mjs
//
// Listens on two free ports (each on 127.0.0.1 and, when available, ::1) and prints one JSON
// line: {"port": A, "xport": B}. Pages are opened as http://localhost:A/…; iframe.html?xport=B
// loads its second frame from http://127.0.0.1:B — another site, so Chromium runs it out of
// process. Loopback only: nothing listens on other interfaces.
//
// Routes besides the static files:
//   GET  /api/items?page=N   {"items":[…],"page":N,"next":N+1|null}
//   GET  /api/items/<id>     {"id","name","tags":[…]}
//   POST /api/items          201 {"id":99, …posted JSON}
//   GET  /api/price          {"price":10}
//   GET  /e2e-ads/*          a script that sets window.adLoaded (adblock must stop it)
//   GET  /hits               {"<path>": count} of every request so far (what got through)

import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(fileURLToPath(new URL("../../fixtures/sites/", import.meta.url)));
const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".txt": "text/plain; charset=utf-8",
  ".csv": "text/csv; charset=utf-8",
};

const hits = {};

function send(res, status, body, type = "application/json") {
  const data = typeof body === "string" || Buffer.isBuffer(body) ? body : JSON.stringify(body);
  res.writeHead(status, {
    "Content-Type": type,
    "Content-Length": Buffer.byteLength(data),
    "Cache-Control": "no-store",
    "Access-Control-Allow-Origin": "*",
  });
  res.end(data);
}

async function readBody(req) {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  return Buffer.concat(chunks).toString("utf8");
}

async function handle(req, res) {
  const url = new URL(req.url ?? "/", "http://fixture");
  const path = decodeURIComponent(url.pathname);
  hits[path] = (hits[path] ?? 0) + 1;

  if (path === "/hits") return send(res, 200, hits);
  if (path === "/api/items" && req.method === "POST") {
    let data = {};
    try {
      data = JSON.parse((await readBody(req)) || "{}");
    } catch {
      return send(res, 400, { error: "bad json" });
    }
    return send(res, 201, { id: 99, ...data });
  }
  if (path === "/api/items") {
    const page = Number(url.searchParams.get("page") ?? "1");
    const items = [0, 1, 2].map((i) => ({ id: page * 10 + i, name: `Item ${page * 10 + i}` }));
    return send(res, 200, { items, page, next: page < 3 ? page + 1 : null });
  }
  const item = /^\/api\/items\/(\d+)$/.exec(path);
  if (item) {
    const id = Number(item[1]);
    return send(res, 200, { id, name: `Item ${id}`, tags: ["new"] });
  }
  if (path === "/api/price") return send(res, 200, { price: 10 });
  // languages.test.ts: the Accept-Language header the browser sent.
  if (path === "/echo/accept-language") {
    return send(res, 200, { accept_language: req.headers["accept-language"] ?? null });
  }
  if (path.startsWith("/e2e-ads/")) {
    return send(res, 200, "window.adLoaded = true;", TYPES[".js"]);
  }

  // Static files, never outside ROOT.
  const file = normalize(join(ROOT, path.endsWith("/") ? `${path}index.html` : path));
  if (file !== ROOT && !file.startsWith(ROOT + sep)) return send(res, 403, { error: "forbidden" });
  try {
    const info = await stat(file);
    if (!info.isFile()) return send(res, 404, { error: "not found" });
    const body = await readFile(file);
    return send(res, 200, body, TYPES[extname(file).toLowerCase()] ?? "application/octet-stream");
  } catch {
    return send(res, 404, { error: "not found" });
  }
}

/** Listen on 127.0.0.1 (free port) and the same port on ::1 if that works. */
function listen() {
  return new Promise((resolveListen, reject) => {
    const v4 = createServer((req, res) => {
      handle(req, res).catch((err) => send(res, 500, { error: String(err) }));
    });
    v4.once("error", reject);
    v4.listen(0, "127.0.0.1", () => {
      const port = v4.address().port;
      const v6 = createServer(v4.listeners("request")[0]);
      // `localhost` may resolve to ::1 first; without IPv6, Chromium falls back to 127.0.0.1.
      v6.once("error", () => resolveListen(port));
      v6.listen(port, "::1", () => resolveListen(port));
    });
  });
}

const port = await listen();
const xport = await listen();
process.stdout.write(JSON.stringify({ port, xport }) + "\n");
