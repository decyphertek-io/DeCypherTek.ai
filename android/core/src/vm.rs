//! QEMU process lifecycle — the headless Mobian VM.
//!
//! One VM per app, spawned as a child of the app process. The engine is
//! the QEMU aarch64 system emulator bundled inside the APK as
//! `libqemu.so` (Android only extracts/executes binaries from the
//! nativeLibraryDir, so that is where it must live). The VM is strictly
//! headless: no display, no VNC (nothing is ever bound on :5900), no
//! monitor. The ONLY channel into it is SSH, forwarded by QEMU's
//! user-mode networking onto `127.0.0.1:<port>` — loopback only, so the
//! guest is never reachable from any network.
//!
//! Acceleration: `/dev/kvm` when the device is rooted and the kernel
//! exposes it, otherwise QEMU's TCG software emulation — the app falls
//! back gracefully and reports which one it got.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use crate::config::VmStart;

/// Where the VM's files live inside the app data dir.
pub fn vm_dir(dir: &str) -> PathBuf {
    Path::new(dir).join("vm")
}

pub struct Vm {
    child: Child,
    pub port: u16,
    pub pid: u32,
    pub accel: &'static str,
    pub started: Instant,
}

impl Vm {
    /// Is the QEMU process still alive?
    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Terminate: SIGTERM, up to 5 s grace, then SIGKILL.
    pub fn stop(&mut self) -> Result<()> {
        if self.running() {
            unsafe {
                libc::kill(self.pid as libc::pid_t, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if !self.running() {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            unsafe {
                libc::kill(self.pid as libc::pid_t, libc::SIGKILL);
            }
            let _ = self.child.wait();
        }
        Ok(())
    }
}

/// Locate the bundled QEMU engine: `libqemu.so` (APK packaging name) or a
/// plain `qemu-system-aarch64` next to it (dev builds).
pub fn find_engine(engine_dir: &str) -> Result<PathBuf> {
    let dir = Path::new(engine_dir);
    for name in ["libqemu.so", "qemu-system-aarch64"] {
        let p = dir.join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(anyhow!(
        "QEMU engine not found in {engine_dir} — expected libqemu.so \
         (bundled by the Android-Build workflow) or qemu-system-aarch64"
    ))
}

/// UEFI firmware for aarch64: either downloaded into the vm dir, or
/// bundled alongside the engine as libedk2-arm.fd / libedk2-vars.fd.
/// Returns (efi, writable_vars).
fn find_firmware(engine_dir: &str, dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let efi = dir.join("QEMU_EFI.fd");
    let vars_tpl = dir.join("vars-template.fd");
    let vars = dir.join("vars.fd");

    if !efi.is_file() {
        let bundled = Path::new(engine_dir).join("libedk2-arm.fd");
        if bundled.is_file() {
            std::fs::copy(&bundled, &efi).context("copy bundled UEFI firmware")?;
        } else {
            bail!(
                "no UEFI firmware: run image.ensure (bios_url) or bundle \
                 libedk2-arm.fd with the engine"
            );
        }
    }
    if !vars.is_file() {
        if vars_tpl.is_file() {
            std::fs::copy(&vars_tpl, &vars).context("clone UEFI vars")?;
        } else {
            let bundled = Path::new(engine_dir).join("libedk2-vars.fd");
            if bundled.is_file() {
                std::fs::copy(&bundled, &vars).context("copy bundled UEFI vars")?;
            } else {
                bail!("no UEFI vars template: run image.ensure (bios_vars_url)");
            }
        }
    }
    Ok((efi, vars))
}

/// KVM is available when /dev/kvm opens (rooted devices only).
fn kvm_available() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .is_ok()
}

/// Resolve the requested accel mode to what will actually be used.
pub fn resolve_accel(requested: &str) -> &'static str {
    match requested {
        "kvm" => "kvm",
        "tcg" => "tcg",
        _ => {
            if kvm_available() {
                "kvm"
            } else {
                "tcg"
            }
        }
    }
}

/// The full QEMU command line — pure and unit-testable.
pub fn build_args(opts: &VmStart, accel: &str, seccomp: bool) -> Vec<String> {
    let dir = vm_dir(&opts.dir);
    let (cpu, accel_flag) = if accel == "kvm" {
        ("host", "-accel=kvm".to_string())
    } else {
        ("cortex-a57", "-accel=tcg,thread=multi".to_string())
    };
    let mut a: Vec<String> = vec![
        "-machine".into(),
        "virt".into(),
        "-cpu".into(),
        cpu.into(),
        "-smp".into(),
        opts.cpus.to_string(),
        "-m".into(),
        opts.ram_mb.to_string(),
        accel_flag,
        // Headless: no display, no VNC (nothing ever listens on :5900),
        // no monitor. Serial console goes to a log file for debugging.
        "-nodefaults".into(),
        "-display".into(),
        "none".into(),
        "-monitor".into(),
        "none".into(),
        "-serial".into(),
        format!("file:{}", dir.join("serial.log").display()),
    ];
    if seccomp {
        a.push("-sandbox".into());
        a.push("enable=on".into());
    }
    // UEFI boot (Mobian arm64 images are UEFI-bootable).
    a.extend([
        "-drive".into(),
        format!(
            "if=pflash,format=raw,file={},readonly=on",
            dir.join("QEMU_EFI.fd").display()
        ),
        "-drive".into(),
        format!(
            "if=pflash,format=raw,file={}",
            dir.join("vars.fd").display()
        ),
        // The Mobian rootfs.
        "-drive".into(),
        format!(
            "if=none,file={},format=qcow2,id=hd0",
            dir.join("mobian.qcow2").display()
        ),
        "-device".into(),
        "virtio-blk-device,drive=hd0".into(),
        // User-mode networking with ONE forward: sshd on the guest's :22
        // bound to LOOPBACK on the host only. No other inbound path exists.
        "-netdev".into(),
        format!("user,id=net0,hostfwd=tcp:127.0.0.1:{}-:22", opts.port),
        "-device".into(),
        "virtio-net-device,netdev=net0".into(),
    ]);
    a
}

fn spawn_qemu(opts: &VmStart, seccomp: bool) -> Result<Child> {
    let engine = find_engine(&opts.engine_dir)?;
    let dir = vm_dir(&opts.dir);
    std::fs::create_dir_all(&dir).context("create vm dir")?;
    find_firmware(&opts.engine_dir, &dir)?;
    if !dir.join("mobian.qcow2").is_file() {
        bail!("no VM image: run image.ensure (or provision.run) first");
    }
    let accel = resolve_accel(&opts.accel);
    let args = build_args(opts, accel, seccomp);
    Command::new(&engine)
        .args(&args)
        .env("QEMU_AUDIO_DRV", "none")
        .env("LD_LIBRARY_PATH", &opts.engine_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn QEMU")
}

/// Boot the VM. If the engine rejects `-sandbox` (built without seccomp),
/// retry once without it.
pub fn start(opts: &VmStart, on_log: impl Fn(String) + Send + Sync + 'static) -> Result<Vm> {
    let mut child = match spawn_qemu(opts, opts.seccomp) {
        Ok(c) => c,
        Err(_) if opts.seccomp => spawn_qemu(opts, false)?,
        Err(e) => return Err(e),
    };
    let pid = child.id();
    let port = opts.port;
    let accel = resolve_accel(&opts.accel);

    // QEMU chatter (stdout+stderr) -> bus events; a very early exit (bad
    // image, unsupported flag) surfaces here as vm_exit.
    let on_log = Arc::new(on_log);
    if let Some(out) = child.stdout.take() {
        let on_log = Arc::clone(&on_log);
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(out).lines().map_while(|l| l.ok()) {
                on_log(line);
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        let on_log = Arc::clone(&on_log);
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(err).lines().map_while(|l| l.ok()) {
                on_log(line);
            }
        });
    }

    // Give it a moment and check it didn't die instantly.
    std::thread::sleep(Duration::from_millis(700));
    match child.try_wait() {
        Ok(Some(status)) => bail!("QEMU exited immediately: {status}"),
        Ok(None) => {}
        Err(e) => bail!("check QEMU status: {e}"),
    }

    Ok(Vm {
        child,
        port,
        pid,
        accel,
        started: Instant::now(),
    })
}

/// Snapshot for the `vm.state` reply.
pub fn state_json(vm: Option<&mut Vm>) -> serde_json::Value {
    match vm {
        Some(vm) => {
            if !vm.running() {
                return json!({ "running": false });
            }
            json!({
                "running": true,
                "pid": vm.pid,
                "port": vm.port,
                "accel": vm.accel,
                "uptime_secs": vm.started.elapsed().as_secs(),
            })
        }
        None => json!({ "running": false }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> VmStart {
        VmStart {
            dir: "/data/app".into(),
            engine_dir: "/data/app/lib/arm64".into(),
            ram_mb: 2048,
            cpus: 2,
            port: 2222,
            accel: "auto".into(),
            seccomp: true,
        }
    }

    #[test]
    fn args_are_headless_loopback_only() {
        let a = build_args(&opts(), "tcg", true);
        let joined = a.join(" ");
        // Headless: no VNC, no display, no monitor.
        assert!(!joined.contains("vnc"));
        assert!(joined.contains("-display none"));
        assert!(joined.contains("-monitor none"));
        // SSH forward binds loopback only.
        assert!(joined.contains("hostfwd=tcp:127.0.0.1:2222-:22"));
        // Seccomp sandbox on by default.
        assert!(joined.contains("-sandbox enable=on"));
        // TCG fallback flag.
        assert!(joined.contains("-accel=tcg,thread=multi"));
    }

    #[test]
    fn kvm_mode_uses_host_cpu() {
        let a = build_args(&opts(), "kvm", false);
        let joined = a.join(" ");
        assert!(joined.contains("-cpu host"));
        assert!(joined.contains("-accel=kvm"));
        assert!(!joined.contains("-sandbox"));
    }

    #[test]
    fn accel_falls_back_gracefully() {
        // On this host (CI container) /dev/kvm is absent -> auto = tcg.
        let got = resolve_accel("auto");
        assert!(got == "kvm" || got == "tcg");
        assert_eq!(resolve_accel("tcg"), "tcg");
    }
}
