//! First-launch provisioning — the whole sequence behind one command.
//!
//! `provision.run` turns a fresh install into a working, locked app:
//!
//! 1. `image.ensure` — the minimal Mobian qcow2 + UEFI firmware land in
//!    `<dir>/vm/` (resumable download, SHA-256 verified).
//! 2. `vm.start` — headless QEMU boots Mobian; SSH is the only forward,
//!    loopback-only.
//! 3. Wait for the VM's one-time first-boot secret: the guest's firstboot
//!    unit generates a random password at first boot and prints it to the
//!    serial console, which QEMU captures into the app-private serial
//!    log. Log in with it once. No credential ships anywhere — not in
//!    this repo, not in the image.
//! 4. Generate the app's ed25519 keypair, install the public key into
//!    `authorized_keys`, rotate the VM user's password to the install
//!    password from the onboarding screen, and retire the firstboot unit.
//! 5. Pin the VM's host key and seal the app-lock profile (SSH key, host
//!    pin, settings snapshot) under the app-lock password — the same
//!    password the user is told to reuse for the agent vault in the
//!    wizard that follows.
//!
//! The VM is left running: the Flutter side then opens the terminal
//! session and pipes the repo's own `scripts/install.sh` into it, so the
//! user completes the agent's walkthrough (and sets the vault password)
//! inside the very terminal they will keep using.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;
use std::path::Path;
use std::time::Duration;

use crate::config::Provision;
use crate::{image, keystore, ssh, vm};

pub async fn run(call_id: u32, opts: &Provision) -> Result<serde_json::Value> {
    let log = move |line: String| {
        crate::bus::log(call_id, &line);
    };

    // 1. Image + firmware (blocking downloads -> run off the async path).
    log("checking VM image…".into());
    let dir = opts.dir.clone();
    let image_url = opts.image_url.clone();
    let image_sha256 = opts.image_sha256.clone();
    let bios_url = opts.bios_url.clone();
    let bios_sha256 = opts.bios_sha256.clone();
    let bios_vars_url = opts.bios_vars_url.clone();
    let progress_call = call_id;
    tokio::task::spawn_blocking(move || {
        image::ensure(
            &dir,
            &image_url,
            image_sha256,
            bios_url,
            bios_sha256,
            bios_vars_url,
            Box::new(move |done, total, detail| {
                let pct = total.map(|t| done as f64 / t.max(1) as f64);
                crate::bus::progress(progress_call, pct, detail);
            }),
        )
    })
    .await
    .map_err(|e| anyhow!("download task: {e}"))?
    .context("prepare VM image")?;
    log("VM image ready".into());

    // 2. Boot (if not already running).
    let running = crate::bus::with_vm(|v| match v {
        Some(vm) => vm.running(),
        None => false,
    });
    if !running {
        log("booting the headless Mobian VM…".into());
        let start = vm::VmStart {
            dir: opts.dir.clone(),
            engine_dir: opts.engine_dir.clone(),
            ram_mb: opts.ram_mb,
            cpus: opts.cpus,
            port: opts.port,
            accel: "auto".into(),
            seccomp: true,
        };
        let started = tokio::task::spawn_blocking(move || {
            vm::start(&start, |line| {
                crate::bus::vm_log(line);
            })
        })
        .await
        .map_err(|e| anyhow!("boot task: {e}"))?
        .context("start QEMU")?;
        log(format!("QEMU up (pid {}, {})", started.pid, started.accel));
        crate::bus::set_vm(started);
    }

    // 3. First login. No credential ships anywhere: the guest's one-shot
    //    firstboot unit generates a random password at first boot and
    //    prints it to the serial console, which QEMU captures into the
    //    app-private serial log — read it, use it once, rotate it. (An
    //    explicit initial_password overrides this for custom images.)
    let first_password = match &opts.initial_password {
        Some(p) => {
            log("using the explicitly configured initial password".into());
            p.clone()
        }
        None => {
            log("waiting for the VM's one-time first-boot secret…".into());
            wait_boot_secret(&opts.dir, 600).await?
        }
    };
    let auth = ssh::Auth {
        host: "127.0.0.1".into(),
        user: opts.user.clone(),
        key: None,
        password: Some(first_password),
        host_key_pin: None,
    };
    let (handle, host_key) = ssh::connect_port(&auth, opts.port, 600)
        .await
        .context("first SSH login")?;
    log("logged in with the one-time first-boot secret".into());

    // 4. App key + password rotation.
    let (pem, pub_line) = ssh::generate_keypair().context("generate app SSH key")?;
    let install_key = format!(
        "mkdir -p ~/.ssh && chmod 700 ~/.ssh && printf '%s\\n' '{pub_line}' >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys"
    );
    let code = ssh::exec(&handle, &install_key, None, move |l| {
        crate::bus::log(call_id, &format!("vm: {l}"));
    })
    .await
    .context("install authorized key")?;
    if code != 0 {
        bail!("authorizing the app key failed (exit {code})");
    }
    log("app SSH key authorized".into());

    let chpasswd = format!(
        "printf '%s:%s' '{}' '{}' | chpasswd",
        opts.user, opts.new_password
    );
    let code = ssh::exec(&handle, &chpasswd, None, move |l| {
        crate::bus::log(call_id, &format!("vm: {l}"));
    })
    .await
    .context("rotate VM password")?;
    if code != 0 {
        bail!("changing the VM password failed (exit {code})");
    }
    log("VM user password rotated to the install password".into());

    // Prove the new credentials work before sealing anything.
    let fresh = ssh::Auth {
        host: "127.0.0.1".into(),
        user: opts.user.clone(),
        key: Some(pem.clone()),
        password: None,
        host_key_pin: Some(host_key.clone()),
    };
    ssh::connect_port(&fresh, opts.port, 30)
        .await
        .context("verify key login")?;
    log("key login verified".into());

    // Retire the firstboot secret generator: the one-time password is
    // worthless now (rotated to the install password, key auth is live),
    // but the unit should never run again either. Also wipe the secret
    // line from the app-private serial log, best-effort.
    let retire = "systemctl disable dct-firstboot.service >/dev/null 2>&1; rm -f /etc/systemd/system/dct-firstboot.service /usr/libexec/dct-firstboot; true";
    let _ = ssh::exec(&handle, retire, None, move |l| {
        crate::bus::log(call_id, &format!("vm: {l}"));
    })
    .await;
    truncate_serial_secret(&opts.dir);
    log("firstboot secret retired (rotated to your install password)".into());

    // 5. Seal the app-lock profile.
    let mut profile = opts.profile_extra.clone();
    if !profile.is_object() {
        profile = json!({});
    }
    profile["ssh_user"] = json!(opts.user);
    profile["ssh_port"] = json!(opts.port);
    profile["ssh_key"] = json!(pem);
    profile["host_key"] = json!(host_key);
    profile["created"] = json!(crate::now_iso8601());
    let lock_path = Path::new(&opts.lock_path).to_path_buf();
    let lock_password = opts.lock_password.clone();
    tokio::task::spawn_blocking(move || keystore::setup(&lock_password, &lock_path, &profile))
        .await
        .map_err(|e| anyhow!("seal task: {e}"))?
        .context("seal app-lock profile")?;
    log("app-lock profile sealed — the install password now unlocks the app".into());

    Ok(json!({
        "host_key": host_key,
        "public_key": pub_line,
        "port": opts.port,
    }))
}

/// The marker the guest's firstboot unit prints to the serial console:
/// `DCT-BOOT-SECRET: <random>` — one line, once per image install.
pub const BOOT_SECRET_PREFIX: &str = "DCT-BOOT-SECRET: ";

/// Extract the most recent boot secret from serial-log content.
pub fn parse_serial_secret(log: &str) -> Option<String> {
    log.lines()
        .rev()
        .find_map(|l| l.strip_prefix(BOOT_SECRET_PREFIX))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Poll `<dir>/vm/serial.log` until the guest's firstboot unit has
/// printed its one-time secret (or the timeout hits).
pub async fn wait_boot_secret(dir: &str, timeout_secs: u64) -> Result<String> {
    let path = vm::vm_dir(dir).join("serial.log");
    let deadline = Duration::from_secs(timeout_secs);
    let started = std::time::Instant::now();
    loop {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Some(secret) = parse_serial_secret(&content) {
                return Ok(secret);
            }
        }
        if started.elapsed() >= deadline {
            bail!("no first-boot secret appeared on the VM's serial console");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// Remove the secret line(s) from the app-private serial log after the
/// secret has been used and rotated. Best-effort: QEMU keeps writing at
/// its own offset, so a partial truncation just means log noise.
pub fn truncate_serial_secret(dir: &str) {
    let path = vm::vm_dir(dir).join("serial.log");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return;
    };
    let cleaned: Vec<&str> = content
        .lines()
        .filter(|l| !l.starts_with(BOOT_SECRET_PREFIX))
        .collect();
    let joined = if content.ends_with('\n') || !cleaned.is_empty() {
        let mut s = cleaned.join("\n");
        if !s.is_empty() {
            s.push('\n');
        }
        s
    } else {
        cleaned.join("\n")
    };
    let _ = std::fs::write(&path, joined);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_encodes_openssh() {
        let (pem, pub_line) = crate::ssh::generate_keypair().unwrap();
        assert!(pem.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert!(pub_line.starts_with("ssh-ed25519 "));
    }

    #[test]
    fn serial_secret_takes_last_line_only() {
        let log = "\
Debian GNU/Linux 13 ttyAMA0\n\
DCT-BOOT-SECRET: firstbootvalue123\n\
login: \n";
        assert_eq!(
            parse_serial_secret(log).as_deref(),
            Some("firstbootvalue123")
        );
        // A regenerated secret from a later boot wins.
        let log = format!("{log}DCT-BOOT-SECRET: secondvalue456\n");
        assert_eq!(parse_serial_secret(&log).as_deref(), Some("secondvalue456"));
        // No secret, no panic, no result.
        assert_eq!(parse_serial_secret("quiet boot\n"), None);
        assert_eq!(parse_serial_secret(""), None);
        // Empty value does not count.
        assert_eq!(parse_serial_secret("DCT-BOOT-SECRET: \n"), None);
    }

    #[test]
    fn truncate_wipes_secret_lines_only() {
        let dir = std::env::temp_dir().join(format!("dct-secret-{}", std::process::id()));
        std::fs::create_dir_all(vm::vm_dir(&dir.to_string_lossy())).unwrap();
        let path = vm::vm_dir(&dir.to_string_lossy()).join("serial.log");
        std::fs::write(&path, "boot line\nDCT-BOOT-SECRET: zzz\nlogin line\n").unwrap();
        truncate_serial_secret(&dir.to_string_lossy());
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(!after.contains("zzz"));
        assert!(after.contains("boot line"));
        assert!(after.contains("login line"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
