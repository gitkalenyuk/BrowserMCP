# BrowserMCP — браузер, яким керують і людина, і будь-який ШІ

## Context
Папка `C:\Users\J0hnD03\Desktop\BrowserMCP` порожня — проєкт з нуля. Мета: щоденний браузер (Windows + macOS),
де **кожна функція — API** для Claude Code / Codex / будь-якого LLM і скриптів: керування без скріншотів
(accessibility-snapshot + ref-и, як vercel-labs/agent-desktop), повний контроль трафіку, запис дій/відео →
кодеген скрипта → прогін 1000× з різними даними, прихований режим, економія ресурсів. Розробник — користувач + ШІ,
Rust новий → дрібні кроки з перевіркою на кожному.

## Зафіксовані рішення
| Тема | Рішення |
|---|---|
| Рушій / мова | Chromium через **CEF**, ядро на **Rust** (`cef` crate від tauri-apps/cef-rs, pin версії), MCP — офіційний `rmcp` |
| ОС | Windows + macOS |
| UI | Svelte 5 web UI в окремому CEF BrowserView (`bmcp://ui/`), англійська, темна/світла за системою + акцент |
| API | MCP (stdio-проксі + Streamable HTTP), REST + WS (JSON-RPC + події), CDP-порт для Playwright/Puppeteer (вимкнений за замовч., ніколи в stealth), TS SDK |
| Безпека API | без токенів; bind 127.0.0.1 + перевірка Host/Origin (allowlist `bmcp://ui`), без CORS, вкладки не можуть звертатися до API-порту. Мережевий доступ — лише майбутній worker-режим (TLS + ключ) |
| Сприйняття ШІ | snapshot з AX-дерева, ref-и `@<snap>:eN` лише інтерактивним, compact-текст (не JSON), skeleton + drill-down, find, diff після дій; скріншот лише на запит |
| Вкладки | вертикальні/горизонтальні, групи, робочі простори (профіль+проксі), split view |
| Клавіатура | Chrome-шорткати, Ctrl+K палітра, Vim-підказки (з ref-ів), `keymap.json` |
| Кілька ШІ | кожен агент — своя кольорова група вкладок + lock; людина може перехопити |
| Вбудований ШІ | чат-панель: Claude Code (`claude -p --input/output-format stream-json`) і Codex (`codex app-server`) з автопідключеним MCP; PTY-термінал (xterm.js); провайдери: Anthropic, Gemini (нативні), OpenAI-сумісний адаптер для OpenAI/OpenRouter/Mistral/DeepSeek/xAI/Groq/Ollama/LM Studio/Custom |
| Трафік | перегляд з тілами + WS-фрейми, HAR, правила, брейкпоінти як Burp, моки, replay, "API сайту → TS-код" |
| Економія | adblock (Brave `adblock`, EasyList), спрощення на домен (JS/картинки/шрифти/3rd-party/анімації), Reader (readability.js), заморожування/вивантаження вкладок, авто-ліміти від RAM/CPU |
| Прихований | інкогніто (in-memory), без слідів автоматизації, профілі-відбитки через CDP Emulation, підгонка TZ/мови/гео під IP проксі, WebRTC-захист, headless (OSR), емуляція мобільних пристроїв; людський ввід — перемикач на профіль (stealth → human). Патчений Chromium — пізня фаза |
| Проксі | HTTP/HTTPS/SOCKS5 з логіном (SOCKS5-auth через локальний форвардер), пули + ротація + перевірка, PAC-правила |
| Запис | відео (кнопка/API) + лог дій + лог трафіку |
| Скрипти | TypeScript у deno_core (**окремий процес-сайдкар** `browsermcp script-host`: краш не валить браузер, той самий SDK що й зовні); Monaco + покроковий дебаг + `.d.ts` для VS Code; плагіни на TS |
| Запис → скрипт | детермінований кодеген → ШІ параметризує `{{vars}}`, додає waits/цикли/extract |
| Прогони | паралельні профілі, CSV/JSON/Excel, черга/ретраї/пауза/звіти, експорт CSV/Excel/JSON/SQLite/webhook/Google Sheets; тригери: вручну/Ctrl+K/API/cron/webhook/URL; `extract(schema)` — ШІ один раз будує селектори → кеш |
| Поломки | self-healing: пауза → ШІ пропонує патч → підтвердження 1 раз → решта прогонів з патчем |
| Сповіщення | UI, системні ОС, Telegram-бот (з кнопкою підтвердження), webhook |
| Файли | автозавантаження в `data/runs/<id>/downloads`, upload з даних, друк у PDF, авто-обробка діалогів |
| Секрети | ШІ має повний доступ; менеджер паролів і ключі провайдерів — **без шифрування** (plain JSON у `data/`) за вибором користувача |
| Імпорт з Chrome | майстер першого запуску, кожен пункт окремо: закладки+історія / паролі (CSV) / кукі (JSON) / нічого |
| Зберігання | portable: усе в `./data` поруч з exe (на macOS — поруч з `.app`; при App Translocation — попередження) |
| Синхронізація | не потрібна. Дистрибуція — для себе |
| Платформи по черзі | **спершу лише Windows**; код macOS (бандл, helpers) відкладено до Фази 11, але платформо-специфічне ховаємо за `cfg`/трейтом з самого початку |
| Репозиторій | локальний git + приватний GitHub; CI на Windows (macOS-джоба — з Фази 11) |
| Оновлення CEF | раз на місяць вручну: `cargo xtask bump-cef` → e2e-тести |
| Нова вкладка | віджети (можна вмикати/вимикати, нові додаватимуться): поле «що зробити?» для ШІ, статус агентів і прогонів (+ патчі на підтвердження), швидкий запуск скриптів, закладки та часті сайти |

## Архітектура
**Процеси:** `browsermcp` (CEF browser process + Rust-ядро + сервери + job runner) · `browsermcp-helper` (CEF renderer/GPU/utility; лише реєстрація схеми) · `browsermcp mcp-stdio` (тонкий stdio→HTTP проксі, сам запускає браузер, CEF не ініціалізує) · `browsermcp script-host` (deno_core, 1 isolate на потік) · `ffmpeg` сайдкар (відео) · дочірні `claude`/`codex`/PTY.

**Потоки:** main = CEF UI thread (`run_message_loop`, однаково на обох ОС) володіє вікнами і `UiState{TabId→CefBrowser}` · CEF IO thread — adblock + статичні правила (`thread_local!`, без блокувань) · tokio pool — API, CDP-сесії, traffic store, jobs, AI · encoder thread.
**Правило мосту:** між потоками ходять лише `TabId`. tokio→UI: `UiDispatcher::run(|ui| …) -> oneshot` поверх `cef::post_task(UI)`. UI/IO→tokio: `mpsc`. Ніколи `block_on` на UI/IO.
**CDP внутрішньо** — in-process `SendDevToolsMessage` + `AddDevToolsMessageObserver` (без порту, `navigator.webdriver=false`).

**Command Registry (серце проєкту):** кожна функція = один `impl Command` (`NAME`, `Args`/`Output` з `schemars`, `Meta{toolset, mutates_tab, palette, default_keys}`). Реєстр автоматично віддає її в MCP (`tab.navigate`→`tab_navigate`, бо Anthropic API не приймає крапки), REST `POST /v1/cmd/{name}` + OpenAPI, WS JSON-RPC, TS SDK (генерується `xtask gen-sdk`), Ctrl+K, keymap, запис дій (крок = виклик команди → кодеген тривіальний). Middleware: валідація → перевірка lock вкладки → аудит → подія `record.step` → виконання → `CmdError`.
MCP за замовч. віддає лише toolset `core` (~25 tools) + `list_commands`/`call_command`; інші — `/mcp?toolsets=net,record`.

**CEF vs CDP:** adblock/статичні правила/моки/проксі/WebRTC/content settings → CEF hooks (`OnBeforeResourceLoad`, `GetResourceHandler`, `RequestContext` prefs). Перегляд трафіку/тіла/WS/HAR/брейкпоінти → CDP `Network.*` + `Fetch.*` (вмикається лише коли потрібно). Snapshot → `Accessibility.getFullAXTree` + `DOMSnapshot` + auto-attach OOPIF. Ввід → `Input.dispatch*` (isTrusted). Емуляція → `Emulation.*` на кожен target до старту. Відео v1 → `Page.startScreencast`→ffmpeg (`h264_mf`/`h264_videotoolbox`/libvpx). Stealth: без `Runtime.enable`, лише isolated worlds.

## Дерево проєкту
```
BrowserMCP/
├─ Cargo.toml  rust-toolchain.toml  .cargo/config.toml  package.json  pnpm-workspace.yaml
├─ crates/
│  ├─ app/          bin browsermcp: main, CLI (run | --headless | mcp-stdio | script-host | run-job), збірка всього
│  ├─ helper/       bin browsermcp-helper: cef::execute_process, реєстрація bmcp:// схеми
│  ├─ core/         ids, модель (Workspace/Profile/Tab/Group/Lock), Command+Registry, Caller, CmdError,
│  │                EventBus, BrowserBackend trait, settings/keymap, portable paths, SQLite (без CEF — чисті тести)
│  ├─ cef/          ЄДИНИЙ крейт з `cef`: App/Client/handlers, Views (вікно, UI-view, content, split, overlay),
│  │                UiDispatcher, RequestContext на профіль, IO-хуки, DevTools transport, OSR/headless
│  ├─ cdp/          типізований CDP-клієнт поверх Transport (in-process | WebSocket), сесії, auto-attach
│  ├─ perception/   AX+DOM злиття, ref-и, compact/skeleton/drill, find, diff, resolve, actionability, людський ввід
│  ├─ browser/      TabManager, навігація, waits, ввід, емуляція, скріншоти, freeze/discard, RAM governor
│  ├─ net/          traffic store, HAR, правила, брейкпоінти, моки, replay, API→код, adblock, simplify, SOCKS5-форвардер
│  ├─ identity/     профілі, відбитки, stealth, WebRTC, проксі-пули, паролі, cookies, імпорт з Chrome, пристрої
│  ├─ record/       лог дій, TS-кодеген, відео-пайплайн
│  ├─ script/       deno_core host (rustyscript на старті), browser.* op, TS transpile, плагіни, userscripts
│  ├─ jobs/         черга (SQLite), ретраї, дані CSV/JSON/Excel, експорт, cron/webhook, self-healing, extract-кеш
│  ├─ ai/           адаптери Claude Code / Codex, провайдери, агентний цикл, групи агентів, PTY, Telegram-сповіщення
│  └─ api/          axum REST + WS + події, rmcp MCP, Host/Origin guard, OpenAPI/tool-list з Registry
├─ xtask/           download-cef, run (копія CEF runtime), bundle (macOS helpers), gen-sdk, fetch-ffmpeg, fetch-filters
├─ ui/              Svelte 5 + Vite: shell (вкладки, omnibox, workspaces, split), панелі (трафік, запис, AI-чат,
│                   термінал, редактор Monaco, прогони), Ctrl+K, vim-hints, майстер імпорту, налаштування
├─ sdk/protocol/    @browsermcp/protocol — ЗГЕНЕРОВАНІ типи/обгортки команд
├─ sdk/ts/          @browsermcp/sdk — WS-транспорт, events, connect(); той самий browser.* що й усередині
├─ assets/          easylist*.txt, readability.js, stealth/*.js, пресети відбитків і пристроїв, іконки
├─ scripts/examples/ приклади скриптів, плагінів, userscripts, job (CSV + скрипт)
├─ tests/fixtures/sites/  локальні тест-сторінки (форми, iframe, shadow DOM, SPA, websocket)
├─ tests/fixtures/cdp/    записані AXTree/DOM JSON для unit-тестів perception (insta)
├─ tests/e2e/       vitest + SDK проти `browsermcp --headless` (MCP, REST, WS, Playwright connectOverCDP)
└─ docs/            architecture.md, threading.md, commands.md (генерується), packaging-macos.md
data/ (runtime, поруч з exe): config/ profiles/<id>/ db/browsermcp.sqlite vault.json recordings/ videos/ har/ scripts/ plugins/ runs/ logs/ runtime/endpoint.json
```
Залежності: `core` ← {cdp, perception, net, identity} ← browser ← {record, script, jobs, ai, api} ← app; `cef` впроваджується як `Arc<dyn BrowserBackend>`. script/jobs/ai ходять лише через Registry.

## Формат snapshot (приклад)
```
snap 7 tab=3 "Sign in – Acme" https://acme.com/login vp=1280x720 y=0/1840
- main
  - heading "Sign in" h1
  - textbox "Email" @7:e3 req
  - textbox "Password" @7:e4 pwd
  - button "Sign in" @7:e6 disabled
  - list "Providers" 12 items, 3 shown → @7:g1
diff 7→8: ~ @7:e6 enabled | + alert "Wrong password" @8:e9
```
Помилки: `{code, message, retry: now|after_wait|after_snapshot|no, hint, candidates?}` — STALE_REF, AMBIGUOUS_TARGET, TIMEOUT, NOT_INTERACTABLE, NOT_FOUND, TAB_LOCKED, PAGE_CRASHED…

## Каталог MCP (простори імен; [MVP] — у першому релізі)
- `tabs_*` list/open/close/activate/lock/unlock [MVP], group/split/freeze
- `nav_*` goto/back/forward/reload/wait [MVP]
- `page_*` snapshot/find/act/act_batch/diff/read/screenshot/dialog [MVP], eval
- `input_*` keys [MVP], mouse/type/humanlike
- `net_*` list/get/search/ws_frames/har_export/rule_add/mock/breakpoint/resume/replay/api_to_code
- `record_*` start/stop/list/codegen · `script_*` list/get/save/run/debug/heal_propose/heal_apply
- `run_*` start/status/cancel/results/export, `trigger_set` · `profile_*` list/open_incognito [MVP], create/delete, `workspace_set`
- `stealth_*` fingerprint_set/proxy_set/webrtc/check/headless/device · `simplify_*` site/reader, `adblock_*`
- `data_*` bookmarks/history/downloads/cookies/passwords/import_chrome · `ai_*` sessions/start/send/stop, `ai_handoff` [MVP]
- `ui_*` notify [MVP]/ask_user/palette/keymap · `system_*` info/toolsets [MVP], events_poll, cdp_port

Правила токен-економії: diff замість snapshot після дій; interactive-only + бюджет ~1500 токенів → skeleton; стабільні ref-и; batch; пагінація з cursor; тіла лише через `net_get maxBytes`; скріншоти лише на запит (JPEG, scale 0.5, кроп по ref); лічильник токенів у бенчмарку.

## Фази (кожен milestone 1–5 днів, закінчується перевіркою)
**0. Інструменти (2–3 дні)** — `git init` + приватний GitHub; VS 2022 Build Tools, rustup MSVC, CMake/Ninja, Node 22 + pnpm, CEF через `export-cef-dir`; workspace + `cargo xtask run`; CI Windows. ✔ вікно з example.com.
**1. Спайки go/no-go (~2 тижні, Windows)** — S1 CEF Views: UI-view + content-view + overlay (найбільший ризик) · S2 UiDispatcher + in-process CDP (`--dump-ax`) · S3 rmcp hello (`claude mcp add browsermcp -- browsermcp.exe mcp-stdio`) · S5 deno_core сайдкар · S6 screencast→ffmpeg. (S4 macOS-бандл → Фаза 11.) ✔ рішення по кожному.
**2. Оболонка MVP (2–3 тижні)** — вкладки, omnibox, шорткати, вертикальні/горизонтальні, сесії, popups/завантаження/контекстне меню/пошук/zoom/дозволи, профілі + інкогніто. ✔ логін у Gmail/GitHub зберігається; кукі профілів ізольовані.
**3. API + MCP + snapshot — MVP (~3 тижні)** — Registry, REST/WS + guard, MCP HTTP + stdio-проксі, snapshot v1 + golden-тести, `page_act` з actionability і diff, skeleton/drill/find/iframe/shadow DOM, групи агентів + lock + takeover, CDP-порт. ✔ Claude Code і Codex проходять 10-задачний бенчмарк без скріншотів; Playwright `connectOverCDP` працює.
**4. Щоденний браузер (3–4 тижні)** — сторінка нової вкладки з віджетами, Ctrl+K, keymap, Vim-hints, закладки/історія, імпорт з Chrome, менеджер паролів, adblock, workspaces/split, freeze + авто-ліміти, Reader/simplify, теми. ✔ тиждень без Chrome.
**5. Вбудований ШІ (2–3 тижні)** — Claude Code, Codex, PTY, провайдери, паралельні агенти.
**6. Трафік (3 тижні)** — лог, HAR, правила, брейкпоінти, моки, replay, API→код.
**7. Запис і кодеген (2–3 тижні)** — лог дій, відео, кодеген + ШІ-параметризація. ✔ записав логін → скрипт → повтор зелений.
**8. Скрипти і плагіни (3–4 тижні)** — SDK, Monaco + дебаг, плагіни, userscript-тригери.
**9. Масові прогони (3 тижні)** — черга, дані, експорт, cron/webhook, extract-кеш, self-healing, Telegram. ✔ 500 рядків × 8 профілів; зламаний селектор лікується одним підтвердженням.
**10. Прихований режим (3 тижні)** — відбитки, проксі-пули/PAC/гео під IP, WebRTC, людський ввід, headless, мобільні пристрої. ✔ CreepJS/BrowserLeaks/sannysoft консистентні, без WebRTC-витоку.
**11. macOS** — `xtask bundle` з 5 helper-бандлами, ad-hoc підпис, portable `.app` + data поруч, App Translocation, macOS-джоба в CI, повний прогін тестів на Mac.
**Пізніше** — розподілені worker-и, CDP-проксі, OSR hardware-відео, патчений Chromium.

## Головні ризики
CEF Views композиція і фокус між view · macOS helper-бандли і App Translocation · deno_core швидко змінюється (pin, старт з rustyscript) · Chromium не вміє SOCKS5-auth (форвардер) · Playwright може не любити CEF-target-и (CDP-проксі) · Windows sandbox у cef-rs.

## Перевірка (наскрізно)
- `cargo test --workspace` (core/perception/net — чисті unit + insta-golden на CDP-фікстурах).
- `pnpm -C tests/e2e test` — vitest через SDK проти `browsermcp --headless` на локальних fixture-сайтах.
- Ручний MVP-тест: `claude mcp add browsermcp -- <path>\browsermcp.exe mcp-stdio` → «відкрий example.com і клікни посилання за ref» → «залогінься на fixtures/login.html».
- Бенчмарк токенів на задачу після кожної фази; регресія >10% — провал.

## Перший крок реалізації
`git init`, Фаза 0 + спайк S1/S2/S3 — до них жодних фіч.
