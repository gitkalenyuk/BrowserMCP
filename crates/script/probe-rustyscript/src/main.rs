//! rustyscript probe for spike S5. Usage:
//!   probe-rustyscript <file.ts> [idle-ms]
//! Prints timings to stderr; optionally sleeps `idle-ms` so memory can be measured.

use std::time::{Duration, Instant};

use rustyscript::serde_json::{Value, json};
use rustyscript::{Error, Module, Runtime, RuntimeOptions, async_callback};

/// Same idea as the real prelude: `browser.ns.method(args)` -> one async call.
const PRELUDE: &str = r#"
globalThis.inputs = {};
const call = (name, args) => rustyscript.async_functions.bmcp_call(name, args ?? null);
globalThis.browser = new Proxy({}, {
  get: (_t, ns) => new Proxy({}, {
    get: (_t2, method) => (args) => call(`${String(ns)}.${String(method)}`, args),
  }),
});
"#;

fn main() -> Result<(), Error> {
    let mut argv = std::env::args().skip(1);
    let path = argv
        .next()
        .expect("usage: probe-rustyscript <file.ts> [idle-ms]");
    let idle_ms: u64 = argv.next().map(|s| s.parse().unwrap_or(0)).unwrap_or(0);

    let t0 = Instant::now();
    let mut rt = Runtime::new(RuntimeOptions {
        timeout: Duration::from_secs(10),
        ..Default::default()
    })?;
    let t_new = t0.elapsed();

    rt.register_async_function(
        "bmcp_call",
        async_callback!(|name: String, args: Value| async move {
            let out = if name == "tabs.list" {
                json!([
                    {"id": "t1", "url": "https://example.com/", "title": "Example Domain"},
                    {"id": "t2", "url": "https://github.com/", "title": "GitHub"}
                ])
            } else {
                json!({"called": name, "args": args})
            };
            Ok::<Value, Error>(out)
        }),
    )?;
    rt.eval::<()>(PRELUDE)?;

    let module = Module::load(&path).expect("read script");
    let t1 = Instant::now();
    // Transpiles TS (rustyscript does it internally with deno_ast), evaluates,
    // and runs the event loop until all promises settle.
    let result = rt.load_modules(&module, vec![]);
    let t_run = t1.elapsed();
    eprintln!(
        "[probe] runtime_new={:?} run={:?} total={:?}",
        t_new,
        t_run,
        t0.elapsed()
    );
    if let Err(e) = result {
        eprintln!("[probe] error: {}", e.as_highlighted(Default::default()));
        std::process::exit(1);
    }
    if idle_ms > 0 {
        std::thread::sleep(Duration::from_millis(idle_ms));
    }
    Ok(())
}
