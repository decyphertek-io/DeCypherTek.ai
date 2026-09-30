#!/usr/bin/env bash
# DeCypherTek.ai — one-command installer.
#
# Termux first, and the ORDER matters: nothing of the agent is
# downloaded into Termux itself. The installer bootstraps a proot
# Debian Linux home first, and every real step happens INSIDE that
# Linux container, in this order:
#   1. apt update
#   2. apt install -y podman podman-docker podman-compose curl gnupg
#      ca-certificates
#   3. LAST: download the latest DeCypherTek.ai release and install it
#      as /usr/local/bin/decyphertek.ai inside Debian
#
# Containers there run on PODMAN, not docker+dockerd: dockerd cannot
# boot reliably under proot on Android kernels (its default storage
# driver wants an overlayfs mount proot will never provide, and the
# daemon lifecycle fights the container session it dies with). Podman
# is daemonless — no daemon, no bootstrapping a proot-safe state. The
# bootstrap writes the vfs storage driver to /etc/containers/
# storage.conf (the proot-safe mode), and `podman-docker` installs a
# docker-compatible CLI, so the agent's @store keeps issuing plain
# `docker` commands unchanged.
#
# The Debian instance is custom-named 'decyphertek'
# (`proot-distro install debian --override-alias decyphertek`), so it
# never collides with or overwrites a Debian proot you installed
# yourself under the plain 'debian' alias — and scripts/uninstall.sh
# removes exactly the 'decyphertek' instance.
#
# Containers can never run in Termux proper (no root, no cgroups); that
# proot Debian is where they live. Back in Termux, typing `decyphertek.ai`
# runs a SOURCED ALIAS (installed into ~/.bashrc — plus the launcher
# script it points to) that drops you straight into the proot Debian and
# launches the agent there. You get a regular terminal that passes
# through everything typed except @-commands — @chat / @code /
# @research / @store wake the agent. MCP tool servers pulled via @store
# run in hardened containers that talk only to the agent (network=none,
# cap-drop=ALL, stdio-only).
#
# On a PC the install is ONE folder: the release binary lands in
# ~/.decyphertek.ai/bin/ — the same ~/.decyphertek.ai that already holds
# vault.dct and staging/, so the whole agent (program + encrypted data)
# is a single directory. A marked, removable PATH block in ~/.bashrc /
# ~/.zshrc makes `decyphertek.ai` launchable from anywhere. What the
# installer DOES along the way is picked by detection gates: Termux
# (TERMUX), macOS (IS_MAC), which Linux flavor (DISTRO_ID — Debian,
# Ubuntu, Fedora, Arch, openSUSE, Alpine …), which package manager
# (PKG_MGR — apt-get / dnf / pacman / zypper / apk), which CPU
# (ARCH -> the release asset). @store's plain `docker` commands are
# served best-effort by daemonless podman + the podman-docker shim —
# the same runtime the agent runs inside its proot home, so there is no
# docker daemon to enable, start or babysit on a working desktop either.
#
# No separate setup step either way: the installer ends by launching
# decyphertek.ai itself, the launcher detects a fresh install, runs the
# walkthrough (OpenRouter/Ollama, the Leash, @store servers are
# optional adds), and drops you in the @-shell after.
# Re-run any time to update — it asks again: do you want the Production
# or the Experimental branch? (Production = releases/latest, the stable
# Prod-Build output; Experimental = the newest dev-adminotaur prerelease.
# Non-interactive runs take Production; pre-pick with `bash -s --
# experimental` or DCT_CHANNEL=experimental.)
# Removal, when you want it: scripts/uninstall.sh (vault kept unless
# --purge).
set -euo pipefail

REPO="decyphertek-io/DeCypherTek.ai"
BIN_NAME="decyphertek.ai"
DATA_DIR="${HOME}/.decyphertek.ai"
PD_ROOT=""    # set on Termux: debian rootfs location
# OUR Debian instance, under its own alias: a Debian proot the user
# installed under the plain 'debian' alias is never touched.
PROOT_DISTRO="decyphertek"

say()  { printf '\033[36m>>\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m ✓\033[0m %s\n' "$*"; }
warn() { printf '\033[33m !\033[0m %s\n' "$*"; }
die()  { printf '\033[31m x\033[0m %s\n' "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1; }

case "$(uname -s)" in
  MSYS*|MINGW*|CYGWIN*) die "Windows is not supported by this installer yet." ;;
esac

# ---------------------------------------------------------------- platform
# Boolean logic gates: what the installer runs on decides what it does.
#   TERMUX=1  — Android/Termux: the agent's home becomes a proot Debian
#               (containers need root-free podman); nothing of the agent
#               is downloaded into Termux itself.
#   IS_MAC=1  — binary + PATH only; runtime hint points at Docker Desktop.
#   PC Linux  — the flavor (DISTRO_ID via /etc/os-release) and its
#               package manager (PKG_MGR) pick the container-runtime
#               package set; the install itself is ONE folder,
#               ~/.decyphertek.ai, everywhere.
OS="$(uname -s)"
ARCH="$(uname -m)"
TERMUX=0
IS_LINUX=0
IS_MAC=0
if [[ "$OS" == Linux ]];  then IS_LINUX=1; fi
if [[ "$OS" == Darwin ]]; then IS_MAC=1; fi
if [[ -n "${TERMUX_VERSION:-}" || "${PREFIX:-}" == *com.termux* ]]; then
  TERMUX=1
  IS_LINUX=1
fi

case "$OS/$ARCH" in
  Linux/aarch64|Linux/arm64) TARGET="aarch64-unknown-linux-musl" ;;
  Linux/x86_64)              TARGET="x86_64-unknown-linux-musl" ;;
  Linux/armv7l|Linux/armv8l) TARGET="armv7-unknown-linux-musleabihf" ;;
  Darwin/arm64)              TARGET="aarch64-apple-darwin" ;;
  Darwin/x86_64)             TARGET="x86_64-apple-darwin" ;;
  *) die "Unsupported platform: $OS/$ARCH — build from source with cargo." ;;
esac
if [[ "$ARCH" == "armv8l" ]]; then
  warn "32-bit userspace on a 64-bit kernel — installing the armv7 build."
fi

# Which Linux flavor (absent on macOS/Termux): this is the diff between
# an apt, a dnf, a pacman, a zypper and an apk install further down.
DISTRO_ID=""
DISTRO_PRETTY=""
if [[ "$IS_LINUX" == 1 && "$TERMUX" != 1 && -r /etc/os-release ]]; then
  DISTRO_ID="$(sed -n 's/^ID=//p' /etc/os-release | head -1 | tr -d '"')"
  DISTRO_PRETTY="$(sed -n 's/^PRETTY_NAME=//p' /etc/os-release | head -1 | tr -d '"')"
fi

# The package-manager gate drives every package install from here on
# (prerequisites and the container runtime), so Debian vs Fedora vs Arch
# each take their own command.
PKG_MGR=""
if [[ "$TERMUX" != 1 ]]; then
  for _m in apt-get dnf yum pacman zypper apk brew; do
    if need "$_m"; then
      PKG_MGR="$_m"
      break
    fi
  done
fi

if [[ "$TERMUX" == 1 ]]; then
  say "Detected: Termux on Android ($ARCH) -> $TARGET"
  say "Install mode: proot-Debian agent home, containers on root-free podman."
elif [[ "$IS_MAC" == 1 ]]; then
  say "Detected: macOS ($ARCH) -> $TARGET"
  say "Install mode: single binary into $DATA_DIR/bin + PATH."
else
  say "Detected: ${DISTRO_PRETTY:-Linux} ($ARCH) -> $TARGET"
  say "Install mode: one folder — $DATA_DIR; runtime packages via ${PKG_MGR:-no package manager detected}."
fi

# ----------------------------------------------------------------- channel
# Which build to install — asked exactly once, prominently:
#   production   — releases/latest: the stable Prod-Build output every
#                  phone and PC gets by default.
#   experimental — the dev prerelease the 'dev-adminotaur' testing
#                  branch publishes (its single rolling
#                  v<version>-dev tag — every push overwrites the
#                  previous build in place).
# Non-interactive runs default to production. Pre-pick a side with the
# first argument (curl -fsSL ... | bash -s -- experimental) or the
# DCT_CHANNEL env var.
CHANNEL="${1:-${DCT_CHANNEL:-}}"
CHANNEL_PROMPT=0
case "$CHANNEL" in
  "") CHANNEL_PROMPT=1 ;;
  production|Production|prod|stable)  CHANNEL="production" ;;
  experimental|Experimental|dev|exp)  CHANNEL="experimental" ;;
  *) die "unknown channel '$CHANNEL' — use 'production' or 'experimental'" ;;
esac
if [[ "$CHANNEL_PROMPT" == 1 ]]; then
  # A terminal worth asking on: interactive stdout + a usable /dev/tty
  # (| bash curl-piping keeps this true; cron/CI falls through cleanly).
  if [[ -t 1 && -r /dev/tty && -w /dev/tty ]]; then
    say "Two channels ship DeCypherTek.ai:"
    echo "  1) Production   — the latest stable release (default: just press Enter)"
    echo "  2) Experimental — the newest dev build from the 'dev-adminotaur' branch"
    ans=""
    printf 'Do you want the Production or Experimental branch? [1 (Prod) / 2 (Dev)] '
    read -r ans </dev/tty || true
    case "${ans:-}" in
      2|e|E|exp*|dev*|Experimental*) CHANNEL="experimental" ;;
      *) CHANNEL="production" ;;
    esac
  else
    CHANNEL="production"
  fi
fi
if [[ "$CHANNEL" == experimental ]]; then
  ok "Channel: experimental — newest 'dev-adminotaur' prerelease build."
else
  ok "Channel: production — latest stable release."
fi

# ------------------------------------------------------------ prerequisites
# Packages go through whichever manager the gates detected — Debian's
# apt, Fedora's dnf, Arch's pacman, openSUSE's zypper, Alpine's apk or
# Homebrew on macOS — so the same installer is portable across flavors.
SU=""
if need sudo; then SU="sudo"; fi

pkg_install() {  # $@ = packages -> rc 0 when the detected manager installed them
  case "$PKG_MGR" in
    apt-get)
      ${SU} apt-get update -qq >/dev/null 2>&1 || true
      ${SU} apt-get install -y -qq "$@" >/dev/null 2>&1 ;;
    dnf)    ${SU} dnf install -y -q "$@" >/dev/null 2>&1 ;;
    yum)    ${SU} yum install -y -q "$@" >/dev/null 2>&1 ;;
    pacman) ${SU} pacman -S --noconfirm --needed "$@" >/dev/null 2>&1 ;;
    zypper) ${SU} zypper --non-interactive install "$@" >/dev/null 2>&1 ;;
    apk)    ${SU} apk add "$@" >/dev/null 2>&1 ;;
    brew)   brew install "$@" >/dev/null 2>&1 ;;
    *)      return 1 ;;
  esac
}

if [[ "$TERMUX" == 1 ]]; then
  say "Termux — updating packages…"
  pkg update -y >/dev/null 2>&1 || warn "pkg update hiccup; continuing"
  for pk in curl tar git openssh rclone coreutils proot-distro; do
    if ! need "$pk"; then
      pkg install -y "$pk" >/dev/null 2>&1 || warn "could not install $pk — continuing"
    fi
  done
  # The release download happens inside the Debian container, so Termux
  # itself needs no curl for the install — only proot-distro is required.
  need proot-distro || die "proot-distro is required — run 'pkg install proot-distro' and re-run."
elif [[ -n "$PKG_MGR" ]] && { ! need curl || ! need tar; }; then
  say "Installing prerequisites (curl, tar) via $PKG_MGR…"
  pkg_install curl tar || warn "could not install prerequisites — continuing"
fi
if [[ "$TERMUX" != 1 ]]; then
  need curl || die "curl is required (install it and re-run)"
  need tar  || die "tar is required (install it and re-run)"
  # checksum tool: Linux uses sha256sum, macOS uses shasum.
  SUMTOOL=""
  if need sha256sum; then
    SUMTOOL="sha256sum"
  elif need shasum; then
    SUMTOOL="shasum -a 256"
  else
    warn "no sha256 tool found — checksum verify will be skipped"
  fi
fi
echo

# =========================================== TERMUX: proot Debian + Podman
# Containers cannot run in Termux itself (no root, no cgroups). The
# agent's home becomes a proot Debian Linux, custom-aliased
# 'decyphertek' so it never collides with a Debian proot the user
# installed by hand. Inside, the runtime is podman — daemonless, with
# `podman-docker` providing the docker-compatible CLI @store already
# speaks — configured to the vfs storage driver because proot cannot
# mount overlayfs. @store's MCP servers then run in hardened containers
# — network=none anyway.
#
# proot-distro v5+ pulls OCI images and keeps the rootfs under
# containers/<name>/rootfs; v4 used plugin tarballs under
# installed-rootfs/<name>. Support both layouts so fresh installs and
# proot-distro upgrades keep working. Debian is first-class in
# proot-distro on every architecture (aarch64, armhf, x86_64), so one
# plain `proot-distro install debian` covers every device — with
# --override-alias giving it the dedicated 'decyphertek' name.

pd_locate_rootfs() {
  PD_ROOT=""
  local cand
  for cand in \
    "${PREFIX}/var/lib/proot-distro/containers/${PROOT_DISTRO}/rootfs" \
    "${PREFIX}/var/lib/proot-distro/installed-rootfs/${PROOT_DISTRO}"; do
    if [[ -d "$cand" ]]; then
      PD_ROOT="$cand"
      return 0
    fi
  done
  return 1
}

# rootfs path of ANY proot-distro alias — used to fingerprint legacy
# instances before adopting them.
pd_rootfs_of() {  # $1 = alias
  local cand
  for cand in \
    "${PREFIX}/var/lib/proot-distro/containers/$1/rootfs" \
    "${PREFIX}/var/lib/proot-distro/installed-rootfs/$1"; do
    if [[ -d "$cand" ]]; then
      printf '%s' "$cand"
      return 0
    fi
  done
  return 1
}

# DeCypherTek's fingerprint inside a proot instance: every installer
# release since the Termux rewrite wrote its launcher/binary there.
ours_by_fingerprint() {  # $1 = rootfs path
  [[ -f "$1/usr/local/bin/dct-launch" ]] || [[ -f "$1/usr/local/bin/$BIN_NAME" ]]
}

# The directory proot-distro itself deletes on removal: v5 keeps the
# rootfs (and its manifest) under containers/<alias>, v4 used
# installed-rootfs/<alias> — a layout v5 does not manage at all.
pd_container_dir_of() {  # $1 = alias -> prints container dir, else rc 1
  local cand
  for cand in \
    "${PREFIX}/var/lib/proot-distro/containers/$1" \
    "${PREFIX}/var/lib/proot-distro/installed-rootfs/$1"; do
    if [[ -d "$cand" ]]; then
      printf '%s' "$cand"
      return 0
    fi
  done
  return 1
}

# Remove an instance ours_by_fingerprint just declared ours. proot-distro
# first — but its 'remove' takes no options in ANY release: the v3/v4
# bash script aborts on them ("got unknown option"), the v5+ rewrite
# exits with "unrecognized option". A '--force' therefore always fails
# while leaving the container installed. When proot-distro still
# refuses — a live agent session holds the container lock (v5+), a lost
# alias plugin makes v4 reject the name, or the binary is simply
# missing — delete the container directory the same way proot-distro
# itself does (best-effort chmod, then rm -rf of the entry alone where
# it is a symlink), plus the v4-era generated alias plugin.
pd_remove_instance() {  # $1 = alias -> rc 0 only when it is gone
  local dir
  if need proot-distro; then
    proot-distro remove "$1" >/dev/null 2>&1 || true
  fi
  if dir="$(pd_container_dir_of "$1")"; then
    if [[ ! -L "$dir" ]]; then
      chmod -R u+rwx "$dir" >/dev/null 2>&1 || true
    fi
    rm -rf "$dir"
    rm -f "${PREFIX}/etc/proot-distro/$1.override.sh"
  fi
  pd_container_dir_of "$1" >/dev/null && return 1
  return 0
}

pd_install_debian() {
  say "Installing the Debian rootfs image (as the '$PROOT_DISTRO' instance)…"
  # Custom alias so OUR container is separate from any user-installed
  # Debian proot: theirs stays untouched, ours is removable by name.
  if ! proot-distro install debian --override-alias "$PROOT_DISTRO" >/dev/null 2>&1; then
    # Older proot-distro builds spelled the flag differently.
    proot-distro install debian --name "$PROOT_DISTRO" >/dev/null 2>&1
  fi
}

# Older installers of this agent used the plain 'debian' alias for THEIR
# container. If such an instance carries our fingerprint it is ours —
# retire it and replace it with the dedicated 'decyphertek' instance. A
# Debian proot without the fingerprint belongs to the user: untouched.
adopt_legacy_debian() {
  local legacy
  if legacy="$(pd_rootfs_of debian)"; then
    if ours_by_fingerprint "$legacy"; then
      say "Found an older DeCypherTek Debian instance under the plain 'debian' alias —"
      say "replacing it with the dedicated '$PROOT_DISTRO' instance…"
      pd_remove_instance debian \
        && ok "previous 'debian' instance removed" \
        || warn "could not remove it — run 'proot-distro remove debian' once."
    else
      warn "a Debian proot named 'debian' exists and is NOT ours — leaving it"
      warn "untouched; the agent gets its own '$PROOT_DISTRO' instance."
    fi
  fi
}

# The Debian-side bootstrap script. Written into the rootfs and run
# INSIDE the container, strictly in this order: apt update first, then
# the podman runtime + its docker-compatible CLI, and the
# DeCypherTek.ai release download LAST — the agent binary is never
# staged in Termux.
dct_write_bootstrap() {
  {
    printf '#!/bin/bash\n'
    printf '# Generated by scripts/install.sh — runs INSIDE the proot Debian.\n'
    printf '# Order matters: apt update first, then the packages, and the\n'
    printf '# DeCypherTek.ai release download LAST.\n'
    printf 'set -euo pipefail\n\n'
    printf 'REPO="%s"\nBIN_NAME="%s"\nCHANNEL="%s"\n' "$REPO" "$BIN_NAME" "$CHANNEL"
    if [[ -n "${GH_TOKEN:-}" || -n "${GITHUB_TOKEN:-}" ]]; then
      printf 'AUTH=(-H "Authorization: Bearer %s")\n' "${GH_TOKEN:-${GITHUB_TOKEN:-}}"
    else
      printf 'AUTH=()\n'
    fi
    cat <<'BOOTSTRAP'

say()  { printf '\033[36m>>\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m ✓\033[0m %s\n' "$*"; }
warn() { printf '\033[33m !\033[0m %s\n' "$*"; }
die()  { printf '\033[31m x\033[0m %s\n' "$*" >&2; exit 1; }

ARCH="$(uname -m)"
case "$ARCH" in
  aarch64|arm64)        TARGET="aarch64-unknown-linux-musl" ;;
  x86_64)               TARGET="x86_64-unknown-linux-musl" ;;
  armv7l|armv8l)        TARGET="armv7-unknown-linux-musleabihf" ;;
  *) die "Unsupported architecture inside Debian: $ARCH — build from source with cargo." ;;
esac

# 1) apt update first.
say "Debian: apt update first…"
apt-get update -qq >/dev/null 2>&1 || warn "apt update hiccup; continuing"

# 2) The container runtime (podman + a docker-compatible CLI for the
#    agent) + TLS certs + https tools inside Debian. No dockerd: podman
#    is daemonless, so there is no daemon to fail at boot under proot.
say "Debian: apt install podman podman-docker podman-compose curl gnupg ca-certificates…"
PKGS=(podman podman-docker podman-compose curl gnupg ca-certificates)
if ! apt-get install -y -qq "${PKGS[@]}" >/dev/null 2>&1; then
  warn "apt could not install the whole set at once — trying package by package…"
  for pk in "${PKGS[@]}"; do
    apt-get install -y -qq "$pk" >/dev/null 2>&1 || warn "could not install $pk — continuing"
  done
fi
command -v curl >/dev/null 2>&1 || die "curl is required inside Debian — re-run the installer."
# podman's default storage driver (overlay) cannot mount overlayfs
# under proot — force the vfs driver: the proot-safe mode.
if command -v podman >/dev/null 2>&1; then
  mkdir -p /etc/containers
  printf '[storage]\ndriver = "vfs"\n' > /etc/containers/storage.conf
fi
ok "Debian: podman podman-docker podman-compose curl gnupg ca-certificates ready"

# 3) LAST — the chosen DeCyphertek.ai release, downloaded and installed
#    here inside the Linux container (not in Termux). CHANNEL arrives
#    from the outer installer: production -> releases/latest,
#    experimental -> the rolling v*-dev prerelease from the testing
#    branch.
GH_API="https://api.github.com/repos/$REPO/releases"
if [[ "$CHANNEL" == experimental ]]; then
  say "Debian: finding the newest experimental dev release…"
  if ! RELIST="$(curl -sSLf "${AUTH[@]}" "$GH_API?per_page=50" 2>/dev/null)"; then
    die "could not list releases — check the network and re-run the installer."
  fi
  TAG="$(printf '%s' "$RELIST" | grep -o '"tag_name": *"[^"]*-dev[0-9a-f.]*"' | head -1 | sed 's/.*"tag_name": *"//; s/"$//')"
  if [[ -z "$TAG" ]]; then
    die "no experimental release found yet — push to the 'dev-adminotaur' branch first, or install the Production channel."
  fi
  say "Debian: downloading the experimental build $TAG (last step)…"
  if ! RELEASE_JSON="$(curl -sSLf "${AUTH[@]}" "$GH_API/tags/$TAG" 2>/dev/null)"; then
    die "could not fetch release $TAG — re-run the installer."
  fi
else
  say "Debian: downloading the latest production release (last step)…"
  if ! RELEASE_JSON="$(curl -sSLf "${AUTH[@]}" "$GH_API/latest" 2>/dev/null)"; then
    die "No releases yet. Trigger the Prod-Build workflow (Actions → Prod-Build → Run workflow) or build with: cargo install --path ."
  fi
  TAG="$(printf '%s' "$RELEASE_JSON" | grep -o '"tag_name": *"[^"]*"' | head -1 | sed 's/.*"tag_name": *"//; s/"$//')"
  if [[ -z "$TAG" ]]; then
    die "No releases yet. Trigger the Prod-Build workflow (Actions → Prod-Build → Run workflow) or build with: cargo install --path ."
  fi
fi
ASSET_URL="$(printf '%s' "$RELEASE_JSON" | grep -o 'https://[^"]*decyphertek-'"$TARGET"'.tar.gz' | head -1)"
if [[ -z "$ASSET_URL" ]]; then
  die "Release $TAG has no asset for $TARGET — see github.com/$REPO/releases"
fi
CHECKSUM_URL="$(printf '%s' "$RELEASE_JSON" | grep -o 'https://[^"]*checksums.txt' | head -1 || true)"

TMPD="$(mktemp -d)"
trap 'rm -rf "$TMPD"' EXIT
curl -sSLf -o "$TMPD/decyphertek.tar.gz" "$ASSET_URL"
if [[ -n "$CHECKSUM_URL" ]]; then
  curl -sSLf -o "$TMPD/checksums.txt" "$CHECKSUM_URL" || warn "checksum list unavailable — skipping verify"
fi
if [[ -f "$TMPD/checksums.txt" ]] && command -v sha256sum >/dev/null 2>&1; then
  EXPECT="$(grep 'decyphertek-'"$TARGET"'.tar.gz' "$TMPD/checksums.txt" | awk '{print $1}')"
  GOT="$(sha256sum "$TMPD/decyphertek.tar.gz" | awk '{print $1}')"
  if [[ -n "$EXPECT" && "$EXPECT" != "$GOT" ]]; then
    die "SHA-256 mismatch (want $EXPECT, got $GOT) — corrupted download; re-run."
  fi
  ok "SHA-256 verified"
fi

mkdir -p "$TMPD/unpack"
tar -xzf "$TMPD/decyphertek.tar.gz" -C "$TMPD/unpack"
if [[ ! -f "$TMPD/unpack/decyphertek" ]]; then
  die "asset did not contain the 'decyphertek' binary"
fi
install -m 0755 "$TMPD/unpack/decyphertek" "/usr/local/bin/$BIN_NAME"
ok "Debian: /usr/local/bin/$BIN_NAME installed ($TAG)"
BOOTSTRAP
  } > "$PD_ROOT/root/dct-bootstrap.sh"
}

termux_proot_home() {
  say "Setting up the proot Debian Linux home for the agent (this is where"
  say "containers — and therefore MCP tool servers from @store — can live)…"

  # Older agent installs used the stock 'debian' alias; retire ours and
  # never touch the user's own.
  adopt_legacy_debian

  if ! pd_locate_rootfs; then
    pd_install_debian \
      || die "proot-distro could not install Debian (instance '$PROOT_DISTRO') — run 'pkg upgrade' and re-run."
    pd_locate_rootfs \
      || die "'$PROOT_DISTRO' rootfs not found after install — re-run to retry."
    ok "debian linux rootfs installed (instance: $PROOT_DISTRO)"
  else
    ok "debian linux rootfs already present (instance: $PROOT_DISTRO)"
  fi

  # Everything below runs inside the Debian container, in order — and
  # the DeCypherTek.ai download is the LAST step.
  dct_write_bootstrap
  say "Inside Debian: apt update, then the podman runtime (podman"
  say "podman-docker podman-compose curl gnupg ca-certificates), then the release download…"
  if ! proot-distro login "$PROOT_DISTRO" -- bash /root/dct-bootstrap.sh; then
    rm -f "$PD_ROOT/root/dct-bootstrap.sh"
    die "the Debian bootstrap failed — run 'pkg upgrade' and re-run the installer."
  fi
  rm -f "$PD_ROOT/root/dct-bootstrap.sh"
  ok "binary deployed inside debian: /usr/local/bin/$BIN_NAME"

  # Launcher inside debian: straight into the agent. Containers there
  # run on podman — daemonless, its CLI already docker-compatible — with
  # the vfs storage driver configured by the bootstrap, so there is no
  # daemon to nurse into a proot-safe state at launch time either.
  mkdir -p "$PD_ROOT/usr/local/bin"
  cat > "$PD_ROOT/usr/local/bin/dct-launch" <<'LAUNCH'
#!/bin/bash
# DeCypherTek on proot Debian: straight into the agent. Containers run
# on podman (daemonless, docker-compatible CLI via podman-docker; the
# vfs storage driver lives in /etc/containers/storage.conf), so no
# daemon needs starting before the agent itself.
exec /usr/local/bin/decyphertek.ai "$@"
LAUNCH
  chmod +x "$PD_ROOT/usr/local/bin/dct-launch"

  # Ask the runtime once inside a proot session — podman is daemonless,
  # so unlike a dockerd setup there is nothing to boot or keep alive:
  # this `docker info` (through podman-docker) just warms up the vfs
  # storage area on first run.
  say "Checking the container runtime inside proot Debian (first run may take a moment)…"
  PROBE="$(proot-distro login "$PROOT_DISTRO" -- bash -c \
    'if timeout 30 docker info >/dev/null 2>&1; then echo ok; else echo off; fi' \
    2>/dev/null || echo off)"
  if [[ "$PROBE" == ok ]]; then
    ok "podman answered inside proot — @store MCP servers fully operational."
  else
    warn "the container runtime did not answer inside proot Debian. The agent"
    warn "itself runs fine; @store lists and registers servers, launching"
    warn "containers needs a device/kernel that lets the runtime start them."
  fi
  echo

  # Termux side: the launcher script on $PREFIX/bin (works everywhere),
  # plus the SOURCED ALIAS the user actually types — `decyphertek.ai`
  # goes from a Termux prompt straight into the proot Debian. proot-distro
  # already binds shared storage (/sdcard) itself — re-binding it only
  # prints an overlap warning, so it is left to proot-distro.
  mkdir -p "$PREFIX/bin"
  cat > "$PREFIX/bin/$BIN_NAME" <<WRAP
#!/data/data/com.termux/files/usr/bin/bash
# DeCypherTek.ai — from Termux straight into the proot Debian home.
# The vault data stays in real Termux home and is bind-mounted in
# (survives container rebuilds); the agent + its containers (podman)
# live inside Debian.
# /sdcard and the rest of shared storage are bound by proot-distro itself.
DATA="\$HOME/.decyphertek.ai"
mkdir -p "\$DATA"
ARGS=(login $PROOT_DISTRO --bind "\$DATA:/root/.decyphertek.ai")
proot-distro "\${ARGS[@]}" -- /usr/local/bin/dct-launch "\$@"
WRAP
  chmod +x "$PREFIX/bin/$BIN_NAME"

  for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
    if [[ "$rc" == "$HOME/.zshrc" && ! -f "$rc" ]]; then continue; fi
    touch "$rc"
    if ! grep -q "alias $BIN_NAME=" "$rc"; then
      printf '\n# DeCypherTek.ai — from a Termux prompt straight into the proot Debian home\nalias %s="%s/bin/%s"\n' \
        "$BIN_NAME" "$PREFIX" "$BIN_NAME" >> "$rc"
    fi
  done
  ok "sourced alias installed: '$BIN_NAME' in ~/.bashrc — run it from Termux"
}

# ------------------------------------------------ container runtime (PC)
# @store pulls, runs and manages MCP tool servers with plain `docker`
# commands. On a PC that CLI comes best-effort from daemonless podman +
# the podman-docker shim — the same runtime the agent runs on inside
# its proot home — so no docker daemon needs enabling, starting or
# babysitting on a working desktop either. The package set is chosen
# by the detected flavor: Debian/Ubuntu (apt) also gets uidmap so
# rootless podman can map subordinate UIDs right away; Fedora (dnf),
# Arch (pacman), openSUSE (zypper), Alpine (apk) ship those defaults.
setup_container_runtime() {
  if [[ "$TERMUX" == 1 ]]; then
    return  # the proot-Debian bootstrap installed podman already
  fi
  if [[ "$IS_MAC" == 1 ]]; then
    if need docker; then
      ok "Docker is present: $(docker --version 2>/dev/null || echo running)"
    else
      warn "Docker is not installed — install Docker Desktop for @store"
      warn "containers; the agent itself runs fine without it."
    fi
    return
  fi
  if need docker; then
    ok "container runtime already present: $(docker --version 2>/dev/null || echo docker)"
    return
  fi
  if [[ -z "$PKG_MGR" ]]; then
    warn "no supported package manager detected — install podman + podman-docker"
    warn "manually if you want @store MCP containers."
    return
  fi
  say "Installing the container runtime via $PKG_MGR (podman, podman-docker, podman-compose)…"
  local pkgs=(podman podman-docker podman-compose uidmap)
  if [[ "$PKG_MGR" != "apt-get" ]]; then
    pkgs=(podman podman-docker podman-compose)
  fi
  if ! pkg_install "${pkgs[@]}"; then
    warn "could not install the whole set at once — trying package by package…"
    local pk
    for pk in "${pkgs[@]}"; do
      pkg_install "$pk" || warn "could not install $pk — continuing"
    done
  fi
  if need docker; then
    ok "docker-compatible CLI ready — @store keeps working unchanged"
    if need timeout && ! timeout 30 docker info >/dev/null 2>&1; then
      warn "'docker info' did not answer yet — on a fresh rootless podman this"
      warn "usually clears after one log-out/log-in; the agent itself is unaffected."
    fi
  else
    warn "docker-compatible CLI not present — @store needs podman + podman-docker."
  fi
}

# ------------------------------------------------------------- release fetch
# Desktop path: fetch + verify + stage the chosen channel's release —
# production -> releases/latest, experimental -> the rolling v*-dev
# prerelease published from the testing branch. On Termux this whole
# step happens INSIDE the Debian container instead (see the bootstrap
# above) — nothing is downloaded into Termux.
fetch_release() {
  GH_API="https://api.github.com/repos/$REPO/releases"
  AUTH=()
  if [[ -n "${GH_TOKEN:-}" || -n "${GITHUB_TOKEN:-}" ]]; then
    AUTH=(-H "Authorization: Bearer ${GH_TOKEN:-$GITHUB_TOKEN}")
  fi
  if [[ "$CHANNEL" == experimental ]]; then
    say "Experimental channel — finding the newest 'dev-adminotaur' prerelease…"
    if ! RELIST="$(curl -sSLf "${AUTH[@]}" "$GH_API?per_page=50" 2>/dev/null)"; then
      die "could not list releases — check the network and re-run the installer."
    fi
    TAG="$(printf '%s' "$RELIST" | grep -o '"tag_name": *"[^"]*-dev[0-9a-f.]*"' | head -1 | sed 's/.*"tag_name": *"//; s/"$//')"
    if [[ -z "$TAG" ]]; then
      die "no experimental release found — push to the 'dev-adminotaur' branch first, or install the Production channel."
    fi
    if ! RELEASE_JSON="$(curl -sSLf "${AUTH[@]}" "$GH_API/tags/$TAG" 2>/dev/null)"; then
      die "could not fetch release $TAG — re-run the installer."
    fi
  else
    say "Production channel — finding the latest stable release…"
    if ! RELEASE_JSON="$(curl -sSLf "${AUTH[@]}" "$GH_API/latest" 2>/dev/null)"; then
      die "No releases yet. Trigger the Prod-Build workflow (Actions → Prod-Build → Run workflow) or build with: cargo install --path ."
    fi
    TAG="$(printf '%s' "$RELEASE_JSON" | grep -o '"tag_name": *"[^"]*"' | head -1 | sed 's/.*"tag_name": *"//; s/"$//')"
    if [[ -z "$TAG" ]]; then
      die "No releases yet. Trigger the Prod-Build workflow (Actions → Prod-Build → Run workflow) or build with: cargo install --path ."
    fi
  fi

  ASSET_URL="$(printf '%s' "$RELEASE_JSON" | grep -o 'https://[^"]*/decyphertek-'"$TARGET"'.tar.gz' | head -1)"
  if [[ -z "$ASSET_URL" ]]; then
    die "Release $TAG has no asset for $TARGET — see github.com/$REPO/releases"
  fi
  CHECKSUM_URL="$(printf '%s' "$RELEASE_JSON" | grep -o 'https://[^"]*checksums.txt' | head -1 || true)"

  say "Downloading DeCypherTek $TAG ($TARGET)…"
  TMPD="$(mktemp -d)"
  trap 'rm -rf "$TMPD"' EXIT
  curl -sSLf -o "$TMPD/decyphertek.tar.gz" "$ASSET_URL"
  if [[ -n "$CHECKSUM_URL" ]]; then
    curl -sSLf -o "$TMPD/checksums.txt" "$CHECKSUM_URL" || warn "checksum list unavailable — skipping verify"
  fi

  if [[ -f "$TMPD/checksums.txt" ]] && [[ -n "$SUMTOOL" ]]; then
    EXPECT="$(grep 'decyphertek-'"$TARGET"'.tar.gz' "$TMPD/checksums.txt" | awk '{print $1}')"
    GOT="$($SUMTOOL "$TMPD/decyphertek.tar.gz" | awk '{print $1}')"
    if [[ -n "$EXPECT" && "$EXPECT" != "$GOT" ]]; then
      die "SHA-256 mismatch (want $EXPECT, got $GOT) — corrupted download; re-run."
    fi
    ok "SHA-256 verified"
  fi

  mkdir -p "$TMPD/unpack"
  tar -xzf "$TMPD/decyphertek.tar.gz" -C "$TMPD/unpack"
  if [[ ! -f "$TMPD/unpack/decyphertek" ]]; then
    die "asset did not contain the 'decyphertek' binary"
  fi
  BIN_STAGED="$TMPD/unpack/decyphertek"
  chmod +x "$BIN_STAGED"
  INSTALLED_VERSION="$("$BIN_STAGED" --version 2>/dev/null || echo unknown)"
}

# ---------------------------------------------------------------- install
# PC: ONE folder. The binary lives in ~/.decyphertek.ai/bin — the same
# ~/.decyphertek.ai that will hold vault.dct and staging/, so program +
# encrypted data is a single directory: an uninstall without --purge
# removes bin/ and keeps the vault; --purge removes everything.
if [[ "$TERMUX" == 1 ]]; then
  termux_proot_home
  LAUNCHER="$PREFIX/bin/$BIN_NAME"
  ok "DeCypherTek.ai ($CHANNEL channel) installed inside proot Debian — '$BIN_NAME' launches it from Termux"
else
  fetch_release
  DEST_BIN="$DATA_DIR/bin"
  mkdir -p "$DEST_BIN"
  mv "$BIN_STAGED" "$DEST_BIN/$BIN_NAME"
  chmod +x "$DEST_BIN/$BIN_NAME"
  LAUNCHER="$DEST_BIN/$BIN_NAME"

  # Older desktop installers of this agent put the binary in
  # ~/.local/bin — retire that exact file so exactly one exists.
  if [[ -e "$HOME/.local/bin/$BIN_NAME" ]]; then
    rm -f "$HOME/.local/bin/$BIN_NAME"
    ok "retired the older ~/.local/bin/$BIN_NAME layout"
  fi

  setup_container_runtime

  # PATH: a marked, removable block — written only when missing.
  if [[ ":$PATH:" != *":$DEST_BIN:"* ]]; then
    for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
      if [[ "$rc" == "$HOME/.zshrc" && ! -f "$rc" ]]; then continue; fi
      touch "$rc"
      if ! grep -qF '.decyphertek.ai/bin' "$rc"; then
        printf '\n# DeCypherTek.ai — agent bin on PATH (uninstall.sh removes this block)\nexport PATH="$HOME/.decyphertek.ai/bin:$PATH"\n' >> "$rc"
      fi
    done
    export PATH="$DEST_BIN:$PATH"
    ok "PATH wired: $DEST_BIN"
  fi
  ok "DeCypherTek.ai $TAG ($CHANNEL channel, $INSTALLED_VERSION) — everything under $DATA_DIR"
fi
echo

# ---------------------------------------------------------------- launch
# No separate setup step: the agent detects whether it has been set up,
# runs the walkthrough itself if not, then lands in the @-shell — a
# regular terminal that passes through everything except @-commands.
# @setup re-runs the walkthrough any time while inside.
if [[ -f "$DATA_DIR/vault.dct" ]]; then
  ok "existing agent found at $DATA_DIR — update complete."
else
  ok "no existing agent — the walkthrough (brain, leash, vault"
  ok "password) starts automatically on first launch. @setup re-runs it."
fi

# The installer finishes by RUNNING decyphertek.ai itself — not by
# telling you to. `curl | bash` pipes the script to stdin (so stdin is
# not a terminal); when stdout is a terminal, hand the agent the real
# tty and launch it here.
if [[ -t 1 ]]; then
  say "Launching DeCypherTek.ai…"
  if [[ -r /dev/tty && -w /dev/tty ]]; then
    exec "$LAUNCHER" </dev/tty
  else
    exec "$LAUNCHER"
  fi
else
  say "Not a terminal — launch it yourself with: $BIN_NAME"
fi
