//! Terminals: real pseudo-terminals (ConPTY on Windows) for the UI's terminal panel.
//!
//! `term.open` starts a shell in a PTY; its output is published as `term.output {id, data}`
//! events (UTF-8, decoded lossily; a character split between two reads is kept for the next
//! chunk) and `term.exit {id, code}` when the shell ends. Input comes back with `term.write`.
//!
//! On Windows, ConPTY starts by asking for the cursor position (`ESC[6n`) and waits for the
//! answer before running the shell. The reader answers that first query itself (and drops it
//! from the output), so a terminal works the same with or without an emulator attached.
//!
//! portable-pty is blocking, so each terminal has two plain threads: one reads output, one
//! waits for the process. After the process exits, the waiter closes the PTY (on Windows the
//! reader only sees EOF then), waits for the reader to drain, and only then sends `term.exit`,
//! so no output arrives after the exit event.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_core::EventBus;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::json;

use crate::proc_tree::ProcessTree;
use crate::session::topics;

/// What `term.list` shows.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TermInfo {
    /// Terminal id, e.g. `t1`.
    pub id: String,
    /// Program that was started.
    pub shell: String,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub pid: Option<u32>,
    /// Exit code once the shell has ended; `null` while it runs.
    pub exit_code: Option<u32>,
}

struct Terminal {
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    /// The shell and everything started in it (closing kills programs still running).
    tree: ProcessTree,
    info: Mutex<TermInfo>,
}

/// Every open terminal. Lives inside the `AiService`.
#[derive(Default)]
pub struct Terminals {
    map: Mutex<BTreeMap<String, Arc<Terminal>>>,
    next: AtomicU64,
}

/// Options for [`Terminals::open`].
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// `pwsh`, `cmd`, `bash`, or a path; `None` = the platform default.
    pub shell: Option<String>,
    /// Extra arguments for the shell (e.g. `["/c", "echo hello"]`).
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The program for a shell name.
pub fn resolve_shell(shell: Option<&str>) -> String {
    let have = |name: &str| which::which(name).is_ok();
    match shell.map(str::trim).filter(|s| !s.is_empty()) {
        Some("pwsh") | None if cfg!(windows) => {
            if have("pwsh") {
                "pwsh.exe".into()
            } else {
                "powershell.exe".into()
            }
        }
        Some("cmd") => "cmd.exe".into(),
        None => std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into()),
        Some(other) => other.into(),
    }
}

impl Terminals {
    pub fn open(&self, events: EventBus, opts: OpenOptions) -> Result<TermInfo, String> {
        let size = PtySize {
            rows: opts.rows.max(1),
            cols: opts.cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|e| format!("cannot open a pseudo-terminal: {e}"))?;

        let shell = resolve_shell(opts.shell.as_deref());
        let cwd = opts
            .cwd
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .or_else(|| std::env::var_os("HOME"))
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| PathBuf::from("."));
        let mut cmd = CommandBuilder::new(&shell);
        cmd.args(&opts.args);
        cmd.cwd(&cwd);
        cmd.env("TERM", "xterm-256color");
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("cannot start {shell}: {e}"))?;
        // Our copy of the slave side must be closed, or the reader never sees EOF (Unix).
        drop(pair.slave);

        let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let id = format!("t{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
        let info = TermInfo {
            id: id.clone(),
            shell,
            cwd: cwd.to_string_lossy().into_owned(),
            cols: size.cols,
            rows: size.rows,
            pid: child.process_id(),
            exit_code: None,
        };
        let term = Arc::new(Terminal {
            master: Mutex::new(Some(pair.master)),
            writer: Mutex::new(Some(writer)),
            killer: Mutex::new(child.clone_killer()),
            tree: ProcessTree::new(child.process_id()),
            info: Mutex::new(info.clone()),
        });
        lock(&self.map).insert(id.clone(), term.clone());

        // Output reader.
        let (drained_tx, drained_rx) = std::sync::mpsc::channel::<()>();
        {
            let (events, id, term) = (events.clone(), id.clone(), term.clone());
            std::thread::Builder::new()
                .name(format!("bmcp-term-{id}-read"))
                .spawn(move || {
                    read_output(reader, &events, &id, &term);
                    let _ = drained_tx.send(());
                })
                .map_err(|e| e.to_string())?;
        }
        // Exit waiter.
        std::thread::Builder::new()
            .name(format!("bmcp-term-{id}-wait"))
            .spawn(move || {
                let code = child.wait().map(|s| s.exit_code()).unwrap_or(1);
                // Close the PTY so the reader gets EOF, then let it flush what is left.
                lock(&term.writer).take();
                lock(&term.master).take();
                let _ = drained_rx.recv_timeout(Duration::from_secs(3));
                lock(&term.info).exit_code = Some(code);
                events.emit(topics::TERM_EXIT, json!({ "id": id, "code": code }));
            })
            .map_err(|e| e.to_string())?;
        Ok(info)
    }

    fn get(&self, id: &str) -> Result<Arc<Terminal>, String> {
        lock(&self.map)
            .get(id)
            .cloned()
            .ok_or_else(|| format!("no terminal '{id}'"))
    }

    /// Send keystrokes / text to the shell.
    pub fn write(&self, id: &str, data: &str) -> Result<(), String> {
        let term = self.get(id)?;
        let mut writer = lock(&term.writer);
        let writer = writer
            .as_mut()
            .ok_or_else(|| format!("terminal {id} has exited"))?;
        writer
            .write_all(data.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|e| format!("cannot write to terminal {id}: {e}"))
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let term = self.get(id)?;
        let master = lock(&term.master);
        let master = master
            .as_ref()
            .ok_or_else(|| format!("terminal {id} has exited"))?;
        master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("cannot resize terminal {id}: {e}"))?;
        let mut info = lock(&term.info);
        info.cols = cols.max(1);
        info.rows = rows.max(1);
        Ok(())
    }

    /// Kill the shell (if still running) and forget the terminal. `term.exit` follows.
    pub fn close(&self, id: &str) -> Result<(), String> {
        let term = lock(&self.map)
            .remove(id)
            .ok_or_else(|| format!("no terminal '{id}'"))?;
        // Kill programs started in the shell too, even if the shell itself already exited.
        term.tree.kill();
        if lock(&term.info).exit_code.is_none() {
            let _ = lock(&term.killer).kill();
        }
        Ok(())
    }

    pub fn list(&self) -> Vec<TermInfo> {
        lock(&self.map)
            .values()
            .map(|t| lock(&t.info).clone())
            .collect()
    }

    /// Kill every shell (browser shutdown).
    pub fn close_all(&self) {
        let ids: Vec<String> = lock(&self.map).keys().cloned().collect();
        for id in ids {
            let _ = self.close(&id);
        }
    }
}

/// ConPTY's startup query: "where is the cursor?".
const CURSOR_QUERY: &str = "[6n";

/// Read until EOF, publishing chunks as `term.output`.
fn read_output(mut reader: Box<dyn Read + Send>, events: &EventBus, id: &str, term: &Terminal) {
    let mut buf = [0u8; 8192];
    let mut carry = Vec::new();
    let mut first = true;
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut text = decode_utf8(&mut carry, &buf[..n]);
                if std::mem::take(&mut first) && cfg!(windows) && text.contains(CURSOR_QUERY) {
                    text = text.replacen(CURSOR_QUERY, "", 1);
                    if let Some(writer) = lock(&term.writer).as_mut() {
                        let _ = writer.write_all(b"[1;1R").and_then(|_| writer.flush());
                    }
                }
                if !text.is_empty() {
                    events.emit(topics::TERM_OUTPUT, json!({ "id": id, "data": text }));
                }
            }
        }
    }
    if !carry.is_empty() {
        let text = String::from_utf8_lossy(&carry).into_owned();
        events.emit(topics::TERM_OUTPUT, json!({ "id": id, "data": text }));
    }
}

/// Decode `carry + chunk` as UTF-8. An incomplete character at the end stays in `carry`;
/// invalid bytes become U+FFFD.
pub fn decode_utf8(carry: &mut Vec<u8>, chunk: &[u8]) -> String {
    carry.extend_from_slice(chunk);
    let mut out = String::new();
    loop {
        match std::str::from_utf8(carry) {
            Ok(text) => {
                out.push_str(text);
                carry.clear();
                return out;
            }
            Err(err) => {
                let valid = err.valid_up_to();
                match err.error_len() {
                    // Incomplete sequence at the end: wait for the next chunk.
                    None => {
                        out.push_str(&String::from_utf8_lossy(&carry[..valid]));
                        carry.drain(..valid);
                        return out;
                    }
                    Some(bad) => {
                        out.push_str(&String::from_utf8_lossy(&carry[..valid + bad]));
                        carry.drain(..valid + bad);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_keeps_split_characters() {
        let mut carry = Vec::new();
        let bytes = "aé€".as_bytes(); // 1 + 2 + 3 bytes
        assert_eq!(decode_utf8(&mut carry, &bytes[..2]), "a");
        assert_eq!(decode_utf8(&mut carry, &bytes[2..4]), "é");
        assert_eq!(decode_utf8(&mut carry, &bytes[4..]), "€");
        assert!(carry.is_empty());
        assert_eq!(decode_utf8(&mut carry, b"x\xffy"), "x\u{fffd}y");
    }

    #[test]
    fn shell_names() {
        assert_eq!(resolve_shell(Some("cmd")), "cmd.exe");
        assert_eq!(resolve_shell(Some("C:\\x\\sh.exe")), "C:\\x\\sh.exe");
        if cfg!(windows) {
            assert!(resolve_shell(None).ends_with(".exe"));
        }
    }

    /// Runs a real shell in a PTY and waits for its output and exit events.
    #[tokio::test(flavor = "multi_thread")]
    async fn pty_echo_hello() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let terms = Terminals::default();
        let (shell, args) = if cfg!(windows) {
            ("cmd", vec!["/c".to_string(), "echo hello".to_string()])
        } else {
            ("/bin/sh", vec!["-c".to_string(), "echo hello".to_string()])
        };
        let info = terms
            .open(
                bus.clone(),
                OpenOptions {
                    shell: Some(shell.into()),
                    args,
                    cols: 80,
                    rows: 24,
                    ..Default::default()
                },
            )
            .unwrap();
        let mut output = String::new();
        let code = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let event = rx.recv().await.unwrap();
                if event.data["id"] != info.id.as_str() {
                    continue;
                }
                match event.topic.as_str() {
                    "term.output" => output.push_str(event.data["data"].as_str().unwrap()),
                    "term.exit" => return event.data["code"].as_u64().unwrap(),
                    _ => {}
                }
            }
        })
        .await
        .expect("no term.exit within 20 s");
        assert!(output.contains("hello"), "output was {output:?}");
        assert_eq!(code, 0);
        assert_eq!(terms.list()[0].exit_code, Some(0));
        assert!(
            terms.write(&info.id, "x").is_err(),
            "writing after exit must fail"
        );
        terms.close(&info.id).unwrap();
        assert!(terms.list().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn interactive_shell_write_resize_close() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let terms = Terminals::default();
        let shell = if cfg!(windows) { "cmd" } else { "/bin/sh" };
        let info = terms
            .open(
                bus.clone(),
                OpenOptions {
                    shell: Some(shell.into()),
                    cols: 100,
                    rows: 30,
                    ..Default::default()
                },
            )
            .unwrap();
        terms.write(&info.id, "echo marker42\r\n").unwrap();
        terms.resize(&info.id, 120, 40).unwrap();
        assert_eq!(terms.list()[0].cols, 120);
        let mut output = String::new();
        tokio::time::timeout(Duration::from_secs(20), async {
            while !output.contains("marker42\r\n") && !output.contains("marker42\n") {
                let event = rx.recv().await.unwrap();
                if event.topic == "term.output" {
                    output.push_str(event.data["data"].as_str().unwrap());
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no echo, output: {output:?}"));
        terms.close(&info.id).unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let event = rx.recv().await.unwrap();
                if event.topic == "term.exit" {
                    break;
                }
            }
        })
        .await
        .expect("no term.exit after close");
    }
}
