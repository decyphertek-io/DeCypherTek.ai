#!/usr/bin/env bash
# Build the minimal Mobian VM image for the DeCypherTek.ai Android app.
#
# A headless, ~800 MB qcow2: Mobian/Debian (arm64) with NOTHING but
# openssh, podman and the kernel — Phosh and every GUI piece stripped,
# because the phone already has a UI and the VM is reached over SSH
# only. The agent's own installer (scripts/install.sh) runs inside this
# VM on first launch and installs the terminal AI unchanged.
#
# Output: decyphertek-mobian-minimal-arm64.qcow2 (+ .sha256)
#
# NO CREDENTIALS ANYWHERE — this is the whole point. The app-side user
# logs in with a password the VM generates ITSELF on first boot (see
# step 5.5): a one-shot firstboot unit invents a random password, prints
# it to the serial console (`DCT-BOOT-SECRET: …`) where the app reads it
# from QEMU's app-private serial log, uses it exactly once to install
# the app's SSH key, then rotates it to the user's install password and
# retires the unit. Nothing credential-shaped exists in this repo, in
# the image, or in the app.
#
# Usage (root or sudo, on Debian/trixie or a Debian container):
#   android/scripts/build-mobian-image.sh [--suite trixie] [--size 8G] [--out dist]
#
# What it does, step by step:
#   1. qemu-img create -f qcow2
#   2. attach via qemu-nbd, partition: 256M ESP + root
#   3. debootstrap <suite> (Mobian is Debian + mobile packages; for a
#      headless VM the Debian base IS the Mobian rootfs minus Phosh —
#      its apt repo is added so mobian-* packages resolve if wanted)
#   4. chroot: openssh-server, podman, linux-image-arm64, grub-efi-arm64
#      (UEFI boot — the VM boots via the bundled EDK2 firmware)
#   5. user 'decyphertek' with a LOCKED password, sshd enabled, a
#      firstboot unit that regenerates SSH host keys (fresh identity per
#      image install, so the app's host-key pin is per-device), and the
#      one-shot firstboot unit that mints the first-boot secret
#   6. detach, checksum, done
set -euo pipefail

SUITE="trixie"
SIZE="8G"
OUT="$(pwd)/dist"
MIRROR="${MIRROR:-http://deb.debian.org/debian}"
MOBIAN_REPO="https://repo.mobian.org/"
MOBIAN_KEY="https://repo.mobian.org/mobian.gpg"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --suite) SUITE="$2"; shift 2 ;;
    --size) SIZE="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    -h|--help) sed -n '2,28p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 1 ;;
  esac
done

say()  { printf '\033[36m>>\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m ✓\033[0m %s\n' "$*"; }
die()  { printf '\033[31m x\033[0m %s\n' "$*" >&2; exit 1; }

[[ $EUID -eq 0 ]] || die "run as root (sudo) — nbd + chroot need it"
for tool in qemu-img qemu-nbd debootstrap sfdisk mkfs.vfat mkfs.ext4 chroot; do
  command -v "$tool" >/dev/null 2>&1 || die "missing tool: $tool (apt-get install qemu-utils debootstrap fdisk dosfstools e2fsprogs)"
done

mkdir -p "$OUT"
IMAGE="$OUT/decyphertek-mobian-minimal-arm64.qcow2"
[[ -f "$IMAGE" ]] && rm -f "$IMAGE"

# ---- 1. create + attach ----
say "creating $SIZE qcow2…"
qemu-img create -f qcow2 "$IMAGE" "$SIZE"

NBD="/dev/nbd0"
modprobe nbd max_part=8 2>/dev/null || true
qemu-nbd --disconnect "$NBD" 2>/dev/null || true
qemu-nbd --connect="$NBD" "$IMAGE"
trap 'qemu-nbd --disconnect "$NBD" >/dev/null 2>&1 || true' EXIT

# ---- 2. partition: ESP + root ----
say "partitioning…"
sfdisk "$NBD" <<EOF
label: gpt
unit: sectors
256M : type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=ESP
    : type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=root
EOF
partprobe "$NBD" || sleep 2
# nbd partitions appear as /dev/nbd0p1/p2 (nbd max_part handles numbering)
ESP="${NBD}p1"
ROOT="${NBD}p2"
[[ -b "$ESP" && -b "$ROOT" ]] || die "partitions did not appear ($ESP / $ROOT)"

mkfs.vfat -n ESP "$ESP"
mkfs.ext4 -L root "$ROOT"

MNT="$(mktemp -d /tmp/dct-mobian.XXXXXX)"
mount "$ROOT" "$MNT"
mkdir -p "$MNT/boot/efi"
mount "$ESP" "$MNT/boot/efi"

cleanup() {
  umount -R "$MNT" 2>/dev/null || true
  rm -rf "$MNT"
  qemu-nbd --disconnect "$NBD" >/dev/null 2>&1 || true
}
trap cleanup EXIT

# ---- 3. debootstrap ----
say "debootstrap $SUITE (arm64)…"
debootstrap --arch=arm64 --variant=minbase "$SUITE" "$MNT" "$MIRROR"

mount -t proc proc "$MNT/proc"
mount --bind /dev "$MNT/dev"
mount --bind /dev/pts "$MNT/dev/pts"
mount --bind /sys "$MNT/sys"

# ---- 4. install + configure inside the chroot ----
say "installing the minimal set (kernel, ssh, podman, grub-efi)…"
chroot "$MNT" tee /etc/apt/sources.list <<EOF
deb $MIRROR $SUITE main
deb $MIRROR $SUITE-updates main
deb http://security.debian.org/debian-security $SUITE-security main
EOF

# Mobian's repo (Mobian = Debian + mobile packages; nothing from it is
# needed for a headless VM, but keeping it makes this image a real
# Mobian rootfs and lets the agent apt-install mobian-* later).
chroot "$MNT" bash -c "curl -fsSL '$MOBIAN_KEY' -o /etc/apt/trusted.gpg.d/mobian.asc || true"
chroot "$MNT" tee -a /etc/apt/sources.list <<EOF
deb $MOBIAN_REPO $SUITE main non-free-firmware
EOF

chroot "$MNT" apt-get update
DEBIAN_FRONTEND=noninteractive chroot "$MNT" apt-get install -y --no-install-recommends \
  linux-image-arm64 \
  openssh-server \
  podman \
  podman-compose \
  curl gnupg ca-certificates \
  grub-efi-arm64 \
  bsdextrautils \
  dbus

# vfs storage driver: the VM is a plain VM (overlay works fine here —
# unlike the proot case), but vfs keeps it simple and robust in 8G.
chroot "$MNT" mkdir -p /etc/containers
chroot "$MNT" tee /etc/containers/storage.conf >/dev/null <<'EOF'
[storage]
driver = "vfs"
runroot = "/run/containers/storage"
graphroot = "/var/lib/containers/storage"
EOF

# ---- 5. user + sshd + firstboot secrets ----
say "creating user (LOCKED password) + firstboot secret generator…"
# The user exists, but the password is locked ('*') — nobody, including
# sshd, can log in until the guest's own firstboot unit mints a random
# password below. No credential is baked into this image.
chroot "$MNT" useradd -m -s /bin/bash -p '*' -G sudo decyphertek 2>/dev/null || true
chroot "$MNT" passwd -l decyphertek >/dev/null 2>&1 || true

chroot "$MNT" sed -i 's/^#\?PermitRootLogin.*/PermitRootLogin no/' /etc/ssh/sshd_config
chroot "$MNT" sed -i 's/^#\?PasswordAuthentication.*/PasswordAuthentication yes/' /etc/ssh/sshd_config
chroot "$MNT" systemctl enable ssh

# ---- 5.5 the one-shot firstboot secret ----
# Generates the VM user's password at FIRST BOOT (not at image build),
# prints it once to the serial console (the app reads it from QEMU's
# serial log), and never runs again. The app then rotates the password
# to the user's install password and retires the unit.
chroot "$MNT" tee /usr/libexec/dct-firstboot >/dev/null <<'_EOF_'
#!/bin/sh
# DeCypherTek firstboot: mint the one-time login secret, print it to
# the serial console, mark done. Self-disabling via the done marker.
set -e
install -d -m 700 /var/lib/dct
if [ -e /var/lib/dct/first-boot-done ]; then
  exit 0
fi
PW="$(tr -dc 'A-Za-z2-9' < /dev/urandom | head -c 40)"
echo "decyphertek:${PW}" | chpasswd
echo "DCT-BOOT-SECRET: ${PW}" > /dev/ttyAMA0
echo "DCT-BOOT-SECRET: ${PW}" > /dev/console
touch /var/lib/dct/first-boot-done
_EOF_
chroot "$MNT" chmod 700 /usr/libexec/dct-firstboot

chroot "$MNT" tee /etc/systemd/system/dct-firstboot.service >/dev/null <<'EOF_'
[Unit]
Description=DeCypherTek first-boot login secret (one-shot)
ConditionPathExists=!/var/lib/dct/first-boot-done
Before=ssh.service
[Service]
Type=oneshot
ExecStart=/usr/libexec/dct-firstboot
[Install]
WantedBy=multi-user.target
EOF_
chroot "$MNT" systemctl enable dct-firstboot.service

# Fresh SSH host identity per installed image: the app pins the host key
# on first connect, so every device gets its own.
chroot "$MNT" rm -f /etc/ssh/ssh_host_*
chroot "$MNT" tee /etc/systemd/system/dct-regen-hostkeys.service >/dev/null <<'EOF'
[Unit]
Description=Regenerate SSH host keys (per-image identity)
ConditionPathExists=!/etc/ssh/ssh_host_ed25519_key
Before=ssh.service
[Service]
Type=oneshot
ExecStart=/usr/bin/ssh-keygen -A
[Install]
WantedBy=multi-user.target
EOF
chroot "$MNT" systemctl enable dct-regen-hostkeys.service

# Serial console on ttyAMA0 — QEMU's -serial log gets boot messages.
chroot "$MNT" systemctl enable serial-getty@ttyAMA0.service 2>/dev/null || true

# ---- 6. UEFI boot via grub ----
say "installing grub (arm64-efi)…"
chroot "$MNT" grub-install --target=arm64-efi --efi-directory=/boot/efi \
  --boot-directory=/boot --no-nvram --removable || \
  die "grub-install failed"
chroot "$MNT" tee /etc/default/grub >/dev/null <<'EOF'
GRUB_DEFAULT=0
GRUB_TIMEOUT=3
GRUB_CMDLINE_LINUX_DEFAULT="quiet console=ttyAMA0"
GRUB_CMDLINE_LINUX=""
EOF
chroot "$MNT" update-grub

# ---- 7. cleanup + detach ----
chroot "$MNT" apt-get clean
umount -R "$MNT"
rm -rf "$MNT"
qemu-nbd --disconnect "$NBD"
trap - EXIT

# ---- 8. shrink + checksum ----
say "shrinking (sparsify)…"
qemu-img convert -O qcow2 -c "$IMAGE" "$IMAGE.tmp" && mv "$IMAGE.tmp" "$IMAGE"

(cd "$OUT" && sha256sum "$(basename "$IMAGE")" > "$(basename "$IMAGE").sha256")
ok "image: $IMAGE ($(du -h "$IMAGE" | cut -f1))"
ok "no credentials shipped: the guest mints a one-time first-boot secret itself"
