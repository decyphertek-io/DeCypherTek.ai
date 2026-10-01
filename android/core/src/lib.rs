//! DeCypherTek.ai — Android core (`libdecyphertek_core.so`).
//!
//! The Flutter app loads this cdylib and talks to it over a tiny JSON
//! bus: [`dvm_command`] enqueues a command and returns its call id,
//! [`dvm_poll_events`] drains replies and pushed events. Everything the
//! app does — booting the headless Mobian VM, SSH-ing into it, running
//! the first-launch install, sealing the app-lock keystore — is one bus
//! command. The terminal AI itself is untouched: it is the same single
//! Rust binary the repo has always shipped, installed *inside* the VM by
//! the repo's own `scripts/install.sh`.
//!
//! Security posture (the reason this app exists):
//!
//! * No VNC. Nothing is ever bound on :5900 — QEMU runs headless with
//!   `-display none`, and the only inbound path is SSH, forwarded by
//!   user-mode networking onto `127.0.0.1:<port>` (loopback only).
//! * SSH auth is the app's ed25519 key; the guest host key is pinned at
//!   provisioning and verified on every connection.
//! * The app-lock profile (SSH key + host pin + settings) is sealed with
//!   AES-256-GCM under an Argon2id key derived from the install
//!   password — the same envelope as the agent's vault.

mod bus;
mod config;
mod image;
mod installer;
mod keystore;
mod ssh;
mod vm;

use std::ffi::{c_char, CStr, CString};

thread_local! {
    static LAST_ERROR: std::cell::RefCell<Option<CString>> = const { std::cell::RefCell::new(None) };
}

fn set_last_error(msg: String) {
    LAST_ERROR.with(|e| {
        e.borrow_mut()
            .replace(CString::new(msg).unwrap_or_default())
    });
}

/// Enqueue a JSON command. Returns the call id (echoed in `done` /
/// `error` / `log` / `progress` events), or 0 if the JSON could not be
/// parsed (see [`dvm_last_error`]).
///
/// # Safety
/// `json` must be a valid NUL-terminated UTF-8 C string for the duration
/// of the call.
#[no_mangle]
pub unsafe extern "C" fn dvm_command(json: *const c_char) -> u32 {
    let parsed = (|| -> Option<serde_json::Value> {
        if json.is_null() {
            set_last_error("null command".into());
            return None;
        }
        let s = CStr::from_ptr(json).to_str().ok()?;
        serde_json::from_str(s).ok()
    })();
    let req = match parsed {
        Some(r) => r,
        None => {
            set_last_error("invalid JSON command".into());
            return 0;
        }
    };
    let bus = bus::bus();
    let id = bus
        .call_seq()
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let result = std::panic::catch_unwind(|| bus::dispatch(id, req));
    if let Err(p) = result {
        set_last_error(format!("panic in command handler: {p:?}"));
        return 0;
    }
    id
}

/// Drain pending events as a JSON array string, or null if none.
/// The caller must free the returned string with [`dvm_free_string`].
#[no_mangle]
pub extern "C" fn dvm_poll_events() -> *mut c_char {
    match bus::bus().drain() {
        Some(s) => match CString::new(s) {
            Ok(c) => c.into_raw(),
            Err(_) => std::ptr::null_mut(),
        },
        None => std::ptr::null_mut(),
    }
}

/// Free a string returned by this library.
///
/// # Safety
/// `s` must originate from [`dvm_poll_events`] and must not be freed twice.
#[no_mangle]
pub unsafe extern "C" fn dvm_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

/// The last error on this thread (never freed; valid until the next call).
#[no_mangle]
pub extern "C" fn dvm_last_error() -> *const c_char {
    LAST_ERROR.with(|e| {
        let mut b = e.borrow_mut();
        b.get_or_insert_with(|| CString::new("no error").unwrap())
            .as_ptr()
    })
}

/// Library version string (never freed).
#[no_mangle]
pub extern "C" fn dvm_version() -> *const c_char {
    static VERSION: OnceLock<CString> = OnceLock::new();
    VERSION
        .get_or_init(|| CString::new(env!("CARGO_PKG_VERSION")).unwrap())
        .as_ptr()
}

use std::sync::OnceLock;

/// UTC timestamp, ISO 8601, no dependencies (civil-from-days algorithm).
pub fn now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ffi_is_stable() {
        let v = unsafe { CStr::from_ptr(dvm_version()) };
        assert_eq!(v.to_str().unwrap(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn command_bus_roundtrip() {
        let id = unsafe {
            dvm_command(
                CString::new(r#"{"cmd":"core.version"}"#)
                    .unwrap()
                    .into_raw(),
            )
        };
        assert!(id > 0);
        let events = dvm_poll_events();
        assert!(!events.is_null());
        let s = unsafe { CStr::from_ptr(events) }
            .to_str()
            .unwrap()
            .to_string();
        unsafe { dvm_free_string(events) };
        let arr: serde_json::Value = serde_json::from_str(&s).unwrap();
        let first = &arr[0];
        assert_eq!(first["id"], serde_json::json!(id));
        assert_eq!(first["event"], "done");
        assert_eq!(
            first["data"]["version"],
            serde_json::json!(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn bad_json_returns_zero() {
        let id = unsafe { dvm_command(CString::new("not json").unwrap().into_raw()) };
        assert_eq!(id, 0);
        let err = unsafe { CStr::from_ptr(dvm_last_error()) };
        assert!(!err.to_str().unwrap().is_empty());
    }

    #[test]
    fn iso8601_shape() {
        let ts = now_iso8601();
        // 2026-10-01T12:34:56Z
        assert_eq!(ts.len(), 20);
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[10..11], "T");
        assert!(ts.ends_with('Z'));
    }
}
