//! Typed JSON schemas for every bus command.
//!
//! The Flutter side sends one JSON object per command (`{"cmd":"vm.start",
//! ...}`) and reads replies/events from the poll queue. These structs are
//! that wire format — serde ignores the `"cmd"` discriminator and any
//! unknown keys, so both sides can evolve fields independently.

use serde::Deserialize;
use serde_json::Value;

/// `lock.setup` — seal the app profile under the install password.
#[derive(Debug, Deserialize)]
pub struct LockSetup {
    pub password: String,
    pub lock_path: String,
    /// Arbitrary profile JSON the app wants sealed (settings snapshot).
    pub profile: Value,
}

/// `lock.unlock` — decrypt the app profile.
#[derive(Debug, Deserialize)]
pub struct LockUnlock {
    pub password: String,
    pub lock_path: String,
}

/// `lock.rekey` — change the app-lock password, profile kept.
#[derive(Debug, Deserialize)]
pub struct LockRekey {
    pub lock_path: String,
    pub old_password: String,
    pub new_password: String,
}

/// `lock.exists` — is the app already provisioned?
#[derive(Debug, Deserialize)]
pub struct LockExists {
    pub lock_path: String,
}

/// `image.ensure` — make sure the Mobian qcow2 + UEFI firmware are present.
#[derive(Debug, Deserialize)]
pub struct ImageEnsure {
    /// App-private data dir; everything lands under `<dir>/vm/`.
    pub dir: String,
    pub url: String,
    pub sha256: Option<String>,
    pub bios_url: Option<String>,
    pub bios_sha256: Option<String>,
    pub bios_vars_url: Option<String>,
}

/// `vm.start` — boot the headless VM.
#[derive(Debug, Clone, Deserialize)]
pub struct VmStart {
    /// App-private data dir (holds `vm/`).
    pub dir: String,
    /// nativeLibraryDir — where the bundled `libqemu.so` engine lives.
    pub engine_dir: String,
    #[serde(default = "default_ram")]
    pub ram_mb: u32,
    #[serde(default = "default_cpus")]
    pub cpus: u32,
    #[serde(default = "default_port")]
    pub port: u16,
    /// "auto" (KVM if available, else TCG) | "kvm" | "tcg".
    #[serde(default = "default_accel")]
    pub accel: String,
    /// QEMU seccomp sandbox (`-sandbox enable=on`). Auto-retried without
    /// it if the engine was built without seccomp support.
    #[serde(default = "default_true")]
    pub seccomp: bool,
}

fn default_ram() -> u32 {
    2048
}
fn default_cpus() -> u32 {
    2
}
fn default_port() -> u16 {
    2222
}
fn default_accel() -> String {
    "auto".to_string()
}
fn default_true() -> bool {
    true
}

/// `term.open` — SSH into the VM and attach a PTY shell.
#[derive(Debug, Deserialize)]
pub struct TermOpen {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    /// OpenSSH private key (PEM), sealed in the lock profile.
    pub key: Option<String>,
    /// Password auth (only used during first-launch provisioning).
    pub password: Option<String>,
    /// Pinned host key (openssh public key line) — connections to any
    /// other host key are refused.
    pub host_key: Option<String>,
    #[serde(default = "default_cols")]
    pub cols: u32,
    #[serde(default = "default_rows")]
    pub rows: u32,
    /// How long to wait for sshd to come up (fresh boot is slow).
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}
fn default_cols() -> u32 {
    80
}
fn default_rows() -> u32 {
    24
}
fn default_timeout() -> u64 {
    300
}

/// `term.write` — keystrokes from the on-screen terminal.
#[derive(Debug, Deserialize)]
pub struct TermWrite {
    pub term: u32,
    /// Base64-encoded raw bytes.
    pub b64: String,
}

/// `term.resize` — the terminal widget changed size.
#[derive(Debug, Deserialize)]
pub struct TermResize {
    pub term: u32,
    pub cols: u32,
    pub rows: u32,
}

/// `term.close` — hang up the shell session.
#[derive(Debug, Deserialize)]
pub struct TermClose {
    pub term: u32,
}

/// `provision.run` — the whole first-launch sequence in one command:
/// download image + firmware, boot the VM, wait for sshd, log in with the
/// image's default credentials, generate the app's SSH key, authorize it,
/// set the VM user's password, pin the host key and seal the app-lock
/// profile. The VM is left running so the terminal can attach right away.
#[derive(Debug, Deserialize)]
pub struct Provision {
    pub dir: String,
    pub engine_dir: String,
    #[serde(default = "default_ram")]
    pub ram_mb: u32,
    #[serde(default = "default_cpus")]
    pub cpus: u32,
    #[serde(default = "default_port")]
    pub port: u16,
    /// VM user the minimal image ships with (default `decyphertek`).
    pub user: String,
    /// Explicit initial password for custom, non-standard images.
    ///
    /// `None` — the normal case — uses the guest's OWN first-boot
    /// secret: the image's one-shot firstboot unit generates a random
    /// password at first boot and prints it to the serial console,
    /// where the app reads it from QEMU's (app-private) serial log. No
    /// credential of any kind ships in the repo or the image.
    #[serde(default)]
    pub initial_password: Option<String>,
    /// The install password from the onboarding UI — becomes the VM
    /// user's new password.
    pub new_password: String,
    /// The app-lock password (same value in the normal flow).
    pub lock_password: String,
    pub lock_path: String,
    pub image_url: String,
    pub image_sha256: Option<String>,
    pub bios_url: Option<String>,
    pub bios_sha256: Option<String>,
    pub bios_vars_url: Option<String>,
    /// Extra profile fields (settings snapshot) sealed into the lock.
    #[serde(default)]
    pub profile_extra: Value,
}
