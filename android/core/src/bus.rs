//! The JSON command bus — the whole FFI surface's engine room.
//!
//! Dart sends one JSON command per call (`dvm_command`), gets back a call
//! id, and polls events (`dvm_poll_events`) as a JSON array of objects.
//! Replies carry the id: `log`, `progress`, `done`, `error`. Terminal
//! output and VM lifecycle changes arrive as pushed events without an id
//! (`out`, `term_closed`, `vm_log`, `vm_exit`).
//!
//! Every command runs on the Rust side's own tokio runtime — the Dart UI
//! thread never blocks on QEMU spawns, Argon2 derivations or downloads.

use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::config;
use crate::{image, keystore, ssh, vm};

pub struct Bus {
    rt: tokio::runtime::Runtime,
    events: Mutex<VecDeque<String>>,
    call_seq: AtomicU32,
    term_seq: AtomicU32,
    vm: Mutex<Option<vm::Vm>>,
    terms: Mutex<HashMap<u32, ssh::Term>>,
}

static BUS: OnceLock<Bus> = OnceLock::new();

pub fn bus() -> &'static Bus {
    BUS.get_or_init(|| Bus {
        rt: tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime"),
        events: Mutex::new(VecDeque::new()),
        call_seq: AtomicU32::new(1),
        term_seq: AtomicU32::new(1),
        vm: Mutex::new(None),
        terms: Mutex::new(HashMap::new()),
    })
}

impl Bus {
    pub fn call_seq(&self) -> &AtomicU32 {
        &self.call_seq
    }

    fn push(&self, event: Value) {
        if let Ok(s) = serde_json::to_string(&event) {
            if let Ok(mut q) = self.events.lock() {
                q.push_back(s);
                // Bound the queue: a flooded UI (or a stalled poller) must
                // not grow memory without limit.
                while q.len() > 8192 {
                    q.pop_front();
                }
            }
        }
    }

    /// Drain all pending events as a JSON array string.
    pub fn drain(&self) -> Option<String> {
        let mut q = self.events.lock().ok()?;
        if q.is_empty() {
            return None;
        }
        let taken: Vec<String> = q.drain(..).collect();
        drop(q);
        let joined = format!("[{}]", taken.join(","));
        Some(joined)
    }
}

// ---- event emitters (free functions for brevity at call sites) ----

pub fn log(call_id: u32, line: &str) {
    bus().push(json!({ "id": call_id, "event": "log", "line": line }));
}

pub fn progress(call_id: u32, pct: Option<f64>, detail: &str) {
    bus().push(json!({
        "id": call_id,
        "event": "progress",
        "pct": pct,
        "detail": detail,
    }));
}

pub fn reply_ok(call_id: u32, data: Value) {
    bus().push(json!({ "id": call_id, "event": "done", "data": data }));
}

pub fn reply_err(call_id: u32, message: &str) {
    bus().push(json!({ "id": call_id, "event": "error", "message": message }));
}

pub fn term_out(term_id: u32, bytes: &[u8]) {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    bus().push(json!({ "event": "out", "term": term_id, "b64": b64 }));
}

pub fn term_closed(term_id: u32) {
    bus().push(json!({ "event": "term_closed", "term": term_id }));
}

pub fn vm_log(line: String) {
    bus().push(json!({ "event": "vm_log", "line": line }));
}

pub fn vm_exit(code: Option<i32>) {
    bus().push(json!({ "event": "vm_exit", "code": code }));
}

// ---- shared state helpers ----

pub fn set_vm(v: vm::Vm) {
    // If one is already running, stop it first (one VM per app).
    let old = bus().vm.lock().map(|mut m| m.replace(v)).ok().flatten();
    if let Some(mut old) = old {
        let _ = old.stop();
    }
}

pub fn with_vm<T>(f: impl FnOnce(Option<&mut vm::Vm>) -> T) -> T {
    let guard = bus().vm.lock();
    match guard {
        Ok(mut m) => f(m.as_mut()),
        Err(_) => f(None),
    }
}

pub fn take_vm() -> Option<vm::Vm> {
    bus().vm.lock().ok().and_then(|mut m| m.take())
}

pub fn next_term_id() -> u32 {
    bus().term_seq.fetch_add(1, Ordering::Relaxed)
}

fn with_term<T>(term_id: u32, f: impl FnOnce(&ssh::Term) -> T) -> Option<T> {
    let guard = bus().terms.lock().ok()?;
    let t = guard.get(&term_id)?;
    Some(f(t))
}

fn remove_term(term_id: u32) -> Option<ssh::Term> {
    bus().terms.lock().ok()?.remove(&term_id)
}

// ---- dispatch ----

/// Route one parsed command. Never blocks the caller: heavy work is
/// spawned onto the runtime.
pub fn dispatch(call_id: u32, req: Value) {
    let cmd = req
        .get("cmd")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    match cmd.as_str() {
        "core.version" => {
            reply_ok(
                call_id,
                json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "russh": "bundled",
                }),
            );
        }

        "lock.setup" => spawn_blocking(call_id, req, |req| {
            let p: config::LockSetup = serde_json::from_value(req)?;
            keystore::setup(&p.password, std::path::Path::new(&p.lock_path), &p.profile)?;
            Ok(json!({ "ok": true }))
        }),

        "lock.unlock" => spawn_blocking(call_id, req, |req| {
            let p: config::LockUnlock = serde_json::from_value(req)?;
            let profile = keystore::unlock(&p.password, std::path::Path::new(&p.lock_path))?;
            Ok(json!({ "profile": profile }))
        }),

        "lock.exists" => spawn_blocking(call_id, req, |req| {
            let p: config::LockExists = serde_json::from_value(req)?;
            Ok(json!({ "exists": keystore::exists(std::path::Path::new(&p.lock_path)) }))
        }),

        "lock.rekey" => spawn_blocking(call_id, req, |req| {
            let p: config::LockRekey = serde_json::from_value(req)?;
            keystore::rekey(
                &p.old_password,
                &p.new_password,
                std::path::Path::new(&p.lock_path),
            )?;
            Ok(json!({ "ok": true }))
        }),

        "image.ensure" => spawn_blocking(call_id, req, |req| {
            let p: config::ImageEnsure = serde_json::from_value(req)?;
            image::ensure(
                &p.dir,
                &p.url,
                p.sha256,
                p.bios_url,
                p.bios_sha256,
                p.bios_vars_url,
                Box::new(|done, total, detail| {
                    progress(0, total.map(|t| done as f64 / t.max(1) as f64), detail)
                }),
            )?;
            Ok(json!({ "ok": true }))
        }),

        "vm.start" => spawn_blocking(call_id, req, |req| {
            let p: config::VmStart = serde_json::from_value(req)?;
            let started = vm::start(&p, vm_log)?;
            let out = json!({ "ok": true, "pid": started.pid, "accel": started.accel });
            set_vm(started);
            Ok(out)
        }),

        "vm.stop" => spawn_blocking(call_id, req, |_| {
            if let Some(mut v) = take_vm() {
                v.stop()?;
                vm_exit(None);
            }
            Ok(json!({ "ok": true }))
        }),

        "vm.state" => {
            let state = with_vm(vm::state_json);
            reply_ok(call_id, state);
        }

        "term.open" => spawn(call_id, req, |call_id, req| async move {
            let p: config::TermOpen = serde_json::from_value(req)?;
            let auth = ssh::Auth {
                host: p.host,
                user: p.user,
                key: p.key,
                password: p.password,
                host_key_pin: p.host_key,
            };
            let (handle, _host_key) = ssh::connect_port(&auth, p.port, p.timeout_secs).await?;
            let id = next_term_id();
            let term = ssh::open_pty(
                handle,
                id,
                p.cols,
                p.rows,
                move |bytes| term_out(id, bytes),
                move || term_closed(id),
            )
            .await?;
            if let Ok(mut terms) = bus().terms.lock() {
                terms.insert(id, term);
            }
            reply_ok(call_id, json!({ "term": id }));
            Ok(())
        }),

        "term.write" => {
            if let Ok(p) = serde_json::from_value::<config::TermWrite>(req) {
                match with_term(p.term, |t| t.write(decode_b64(&p.b64))) {
                    Some(Ok(())) => reply_ok(call_id, json!({ "ok": true })),
                    Some(Err(e)) => reply_err(call_id, &e.to_string()),
                    None => reply_err(call_id, "no such terminal session"),
                }
            } else {
                reply_err(call_id, "bad term.write payload");
            }
        }

        "term.resize" => {
            if let Ok(p) = serde_json::from_value::<config::TermResize>(req) {
                match with_term(p.term, |t| t.resize(p.cols, p.rows)) {
                    Some(Ok(())) => reply_ok(call_id, json!({ "ok": true })),
                    Some(Err(e)) => reply_err(call_id, &e.to_string()),
                    None => reply_err(call_id, "no such terminal session"),
                }
            } else {
                reply_err(call_id, "bad term.resize payload");
            }
        }

        "term.close" => {
            if let Ok(p) = serde_json::from_value::<config::TermClose>(req) {
                if let Some(t) = remove_term(p.term) {
                    let _ = t.close();
                }
                reply_ok(call_id, json!({ "ok": true }));
            } else {
                reply_err(call_id, "bad term.close payload");
            }
        }

        "provision.run" => spawn(call_id, req, |call_id, req| async move {
            let p: config::Provision = serde_json::from_value(req)?;
            let data = crate::installer::run(call_id, &p).await?;
            reply_ok(call_id, data);
            Ok::<(), anyhow::Error>(())
        }),

        other => reply_err(call_id, &format!("unknown command: {other}")),
    }
}

fn decode_b64(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .unwrap_or_default()
}

/// Spawn a blocking (sync) command on its own thread — the caller (the
/// Dart UI thread) returns immediately; the reply arrives via events.
fn spawn_blocking<F>(call_id: u32, req: Value, f: F)
where
    F: FnOnce(Value) -> anyhow::Result<Value> + Send + 'static,
{
    std::thread::spawn(move || match f(req) {
        Ok(data) => reply_ok(call_id, data),
        Err(e) => reply_err(call_id, &format!("{e:#}")),
    });
}

/// Spawn an async command onto the runtime.
fn spawn<F, Fut>(call_id: u32, req: Value, f: F)
where
    F: FnOnce(u32, Value) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
{
    bus().rt.spawn(async move {
        if let Err(e) = f(call_id, req).await {
            reply_err(call_id, &format!("{e:#}"));
        }
    });
}
