//! The data side: `rsmm editor --stdio`, run as a child process.
//!
//! The editor never decodes cooked game files itself. Every read and every
//! save goes through the same Python routes the browser editor uses
//! (`src/rsmm/cli/editor/maps.py`), framed over the child's stdin/stdout (see
//! `src/rsmm/cli/editor/stdio.py` for the wire format). That keeps one source
//! of truth for what a map mod is and what proves it cooks.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, bail, Context as _, Result};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub ctype: String,
    pub body: Vec<u8>,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json(&self) -> Result<serde_json::Value> {
        serde_json::from_slice(&self.body).context("response is not JSON")
    }

    /// The `{"error": ...}` message a failed route answers with, or the status.
    pub fn error_text(&self) -> String {
        self.json()
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_owned))
            .unwrap_or_else(|| format!("HTTP {}", self.status))
    }
}

#[derive(Deserialize)]
struct Header {
    id: i64,
    status: u16,
    ctype: String,
    len: usize,
}

type Waiters = Arc<Mutex<HashMap<u64, Sender<Response>>>>;

/// A cheap, clonable handle to the running `rsmm editor --stdio`.
#[derive(Clone)]
pub struct Backend {
    stdin: Arc<Mutex<ChildStdin>>,
    waiters: Waiters,
    next: Arc<AtomicU64>,
    pub hello: Arc<serde_json::Value>,
    /// How rsmm is run (for one-off commands beside the `--stdio` backend).
    pub cmd: Vec<String>,
    pub log: Arc<Mutex<Vec<String>>>,
    _child: Arc<Mutex<Child>>,
}

impl Backend {
    pub fn start(cmd: &[String]) -> Result<Backend> {
        let (prog, pre) = cmd.split_first().ok_or_else(|| anyhow!("empty rsmm command"))?;
        let mut child = Command::new(prog)
            .args(pre)
            .args(["editor", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("could not start {prog}"))?;
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::with_capacity(1 << 20, child.stdout.take().unwrap());
        let stderr = child.stderr.take().unwrap();

        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        {
            let log = log.clone();
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let mut l = log.lock().unwrap();
                    l.push(line);
                    let excess = l.len().saturating_sub(2000);
                    l.drain(..excess);
                }
            });
        }

        // Frame 0 is the hello: where the game and the mods folder are.
        let (h, body) = read_frame(&mut stdout).context("rsmm editor --stdio did not start")?;
        if h.id != 0 {
            bail!("rsmm editor --stdio: expected the hello frame, got id {}", h.id);
        }
        let hello: serde_json::Value = serde_json::from_slice(&body)?;

        let waiters: Waiters = Arc::default();
        {
            let waiters = waiters.clone();
            let log = log.clone();
            thread::spawn(move || loop {
                match read_frame(&mut stdout) {
                    Ok((h, body)) => {
                        let tx = if h.id > 0 { waiters.lock().unwrap().remove(&(h.id as u64)) } else { None };
                        match tx {
                            Some(tx) => {
                                let _ = tx.send(Response { status: h.status, ctype: h.ctype, body });
                            }
                            None => log.lock().unwrap().push(format!(
                                "[editor] unmatched frame {} ({}): {}",
                                h.id,
                                h.status,
                                String::from_utf8_lossy(&body[..body.len().min(200)])
                            )),
                        }
                    }
                    Err(e) => {
                        log.lock().unwrap().push(format!("[editor] backend stopped: {e:#}"));
                        // Fail everything still waiting so no loader hangs.
                        waiters.lock().unwrap().clear();
                        return;
                    }
                }
            });
        }

        Ok(Backend {
            stdin: Arc::new(Mutex::new(stdin)),
            waiters,
            next: Arc::new(AtomicU64::new(1)),
            hello: Arc::new(hello),
            cmd: cmd.to_vec(),
            log,
            _child: Arc::new(Mutex::new(child)),
        })
    }

    /// Send a request; the answer arrives on the returned channel.
    pub fn send(&self, method: &str, target: &str, body: Option<&serde_json::Value>) -> Result<Receiver<Response>> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        self.waiters.lock().unwrap().insert(id, tx);
        let mut req = serde_json::json!({"id": id, "method": method, "target": target});
        if let Some(b) = body {
            req["body"] = b.clone();
        }
        let mut line = serde_json::to_vec(&req)?;
        line.push(b'\n');
        let mut w = self.stdin.lock().unwrap();
        if let Err(e) = w.write_all(&line).and_then(|_| w.flush()) {
            self.waiters.lock().unwrap().remove(&id);
            return Err(e).context("the rsmm backend is not running");
        }
        Ok(rx)
    }

    /// Blocking request, for loader threads.
    pub fn call(&self, method: &str, target: &str, body: Option<&serde_json::Value>) -> Result<Response> {
        self.send(method, target, body)?
            .recv()
            .map_err(|_| anyhow!("the rsmm backend stopped while answering {target}"))
    }

    /// Blocking GET that must succeed with JSON.
    pub fn get_json(&self, target: &str) -> Result<serde_json::Value> {
        let r = self.call("GET", target, None)?;
        if !r.ok() {
            bail!("{target}: {}", r.error_text());
        }
        r.json()
    }
}

fn read_frame(r: &mut impl BufRead) -> Result<(Header, Vec<u8>)> {
    let mut line = Vec::new();
    if r.read_until(b'\n', &mut line)? == 0 {
        bail!("end of stream");
    }
    let h: Header = serde_json::from_slice(&line).with_context(|| {
        format!("bad frame header: {}", String::from_utf8_lossy(&line[..line.len().min(200)]))
    })?;
    let mut body = vec![0u8; h.len];
    r.read_exact(&mut body)?;
    Ok((h, body))
}

/// Percent-encode a query value.
pub fn q(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// How to run rsmm: `--rsmm <cmd>`, `$RSMM_BIN`, the CLI bundled next to this
/// binary, the `./rsmm` wrapper of a repo checkout above the working
/// directory, else `rsmm` on PATH.
pub fn find_rsmm(arg: Option<String>) -> Vec<String> {
    if let Some(a) = arg.or_else(|| std::env::var("RSMM_BIN").ok()).filter(|s| !s.is_empty()) {
        return vec![a];
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["rsmm.exe", "rsmm"] {
                let p = dir.join(name);
                if p.is_file() {
                    return vec![p.to_string_lossy().into_owned()];
                }
            }
        }
    }
    if let Ok(mut d) = std::env::current_dir() {
        loop {
            if let Some(cmd) = repo_wrapper(&d) {
                return cmd;
            }
            if !d.pop() {
                break;
            }
        }
    }
    vec!["rsmm".into()]
}

fn repo_wrapper(d: &Path) -> Option<Vec<String>> {
    let wrapper = d.join("rsmm");
    if !(wrapper.is_file() && d.join("src/rsmm/cli").is_dir()) {
        return None;
    }
    // Prefer the checkout's venv so the CLI runs with its own interpreter.
    for py in [".venv/bin/python", ".venv/Scripts/python.exe"] {
        let p: PathBuf = d.join(py);
        if p.is_file() {
            return Some(vec![p.to_string_lossy().into_owned(), wrapper.to_string_lossy().into_owned()]);
        }
    }
    Some(vec![wrapper.to_string_lossy().into_owned()])
}
