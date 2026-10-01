# DeCypherTek.ai for Android

> A Flutter + Rust app that boots a **headless Mobian VM (QEMU)** on the
> phone and gives you the terminal AI inside it — over **localhost-only
> SSH**. No VNC, no port 5900, nothing on the network.

The existing terminal AI (the repo root's Rust crate) is **not modified
and not re-compiled for Android**: it keeps running on regular
Linux/macOS/Termux exactly as before. This app is a separate build that
provides a real Debian-based virtual machine — solving the
termux/proot limitations (no real kernel, no containers, no systemd) —
and installs that same agent *inside* the VM using the repo's own
`scripts/install.sh`.

## Why

| | Termux + proot | Podroid | **This app** |
| --- | --- | --- | --- |
| Real Linux kernel | no (syscall translation) | yes (QEMU) | **yes (QEMU)** |
| Containers (podman) | vfs-only, fragile | yes | **yes** |
| VNC on :5900 | n/a | **exposed, no password** | **never used** |
| Remote access | n/a | reachable from networks | **loopback-only SSH** |
| GUI in the VM | n/a | Phosh desktop | **none — terminal only** |

The VM is booted by QEMU with `-display none -monitor none` and exactly
one network forward: `hostfwd=tcp:127.0.0.1:2222-:22`. The only way in
is SSH, and the only SSH client is the app itself.

## The pieces

```
android/
├── core/                    Rust cdylib — the app's engine room
│   └── src/
│       ├── lib.rs           FFI surface: dvm_command / dvm_poll_events
│       ├── bus.rs           JSON command bus + event queue
│       ├── vm.rs            QEMU process lifecycle (headless, seccomp,
│       │                   KVM→TCG fallback, loopback-only forward)
│       ├── ssh.rs           russh client: PTY sessions, exec, keygen,
│       │                   host-key pinning
│       ├── image.rs         resumable, SHA-256-verified downloads
│       ├── installer.rs     the whole first-launch sequence
│       ├── keystore.rs      app-lock: AES-256-GCM + Argon2id envelope
│       └── config.rs        the JSON wire schemas
├── app/                     Flutter app
│   └── lib/
│       ├── core/            FFI bindings + typed bus client
│       ├── services/        lock, VM, settings, terminal session
│       └── screens/         onboarding, lock screen, terminal, settings
├── scripts/
│   ├── build-qemu-android.sh    QEMU aarch64 emulator → libqemu.so
│   ├── build-mobian-image.sh    minimal Mobian qcow2 (~800 MB)
│   └── build-android.sh         core + engine + Flutter → APK
└── android/                 the Android platform scaffold (gradle)
```

## First launch (the install password)

1. The app downloads the minimal Mobian image + UEFI firmware
   (resumable, checksummed) and boots the VM.
2. No credential exists yet — the guest's own one-shot firstboot unit
   mints a random password at first boot and prints it to the serial
   console, where the app reads it from QEMU's app-private serial log.
   The app logs in once with that one-time secret, generates the app's
   ed25519 SSH key, authorizes it, rotates the VM user's password to
   the **install password** you typed, retires the firstboot unit,
   wipes the secret from the serial log, pins the VM's host key, and
   seals all of it into the **app-lock profile** — AES-256-GCM under an
   Argon2id key derived from that same password (the same envelope as
   the agent's vault). Nothing credential-shaped is shipped anywhere —
   not in the repo, not in the image, not in the app.
3. The terminal opens and runs the repo's own installer
   (`scripts/install.sh`) inside the VM — you finish the agent's
   walkthrough right there. Use the **same password** for the vault and
   the app stays consistent: one password unlocks the app, the VM and
   the agent's encrypted memory.

From then on, that password is the app lock: it decrypts the profile
that holds the SSH key, so no password — no app, no VM, no data.

## Security posture

- **No VNC.** QEMU runs with no display, no VNC, no monitor. Port 5900
  is never bound — the Podroid flaw does not exist here.
- **Loopback-only.** The single SSH forward binds `127.0.0.1` on the
  phone; no other process on any network can reach the VM.
- **Keyed SSH.** The app authenticates with its own ed25519 key (sealed
  in the lock profile); the VM's host key is pinned at provisioning and
  verified on every connection.
- **QEMU sandbox.** `-sandbox enable=on` (seccomp) when the engine
  supports it, auto-retried without it otherwise.
- **Encrypted at rest.** The lock profile (SSH key, host pin, settings)
  is AES-256-GCM + Argon2id — no key stored, no recovery, by design.
- **No Android permissions.** The APK requests zero permissions; all
  storage is app-private.

## Building

Everything is manual-dispatch CI, separate from the terminal AI's
pipelines (see `.github/workflows/instructions.md`):

1. **VM-Engine-Build** — cross-compiles QEMU (+EDK2 firmware) for
   Android arm64 → the rolling `vm-engine-latest` release. Run rarely.
2. **Android-Build** — gates (root crate unchanged + core tests), then
   builds the APK → the rolling `android-latest` prerelease.

Locally:

```bash
# engine (once per engine change)
android/scripts/build-qemu-android.sh --ndk $ANDROID_NDK_HOME

# minimal Mobian image (root, on Debian) — attach to a release
sudo android/scripts/build-mobian-image.sh

# APK
android/scripts/build-android.sh --engine-release vm-engine-latest
```

The image build script is the one artifact the CI does not produce
(it needs root + nbd): run it on a Debian machine and attach
`decyphertek-mobian-minimal-arm64.qcow2` to a release; the app's
settings default `image_url` points at `releases/latest`.
