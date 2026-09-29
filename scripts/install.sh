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
# On Linux/macOS: same single binary on PATH as `decyphertek.ai`, plus a
# best-effort Docker setup so @store works there too.
#
# No separate setup step either way: the installer ends by launching
# decyphertek.ai itself, the launcher detects a fresh install, runs the
# walkthrough (OpenRouter/Ollama, the Leash, @store servers are
# optional adds), and drops you in the @-shell after.
# Re-run any time to update to the latest release.
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

case "$(uname -s)" in
  MSYS*|MINGW*|CYGWIN*) die "Windows is not supported by this installer yet." ;;
esac

# ---------------------------------------------------------------- platform
OS="$(uname -s)"
ARCH="$(uname -m)"
TERMUX=0
if [[ -n "${TERMUX_VERSION:-}" || "${PREFIX:-}" == *com.termux* ]]; then TERMUX=1; fi

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

say "DeCypherTek.ai installer — $OS/$ARCH -> $TARGET"

# ------------------------------------------------------------ prerequisites
need() { command -v "$1" >/dev/null 2>&1; }

if [[ "$TERMUX" == 1 ]]; then
  say "Termux detected — updating packages…"
  pkg update -y >/dev/null 2>&1 || warn "pkg update hiccup; continuing"
  for pk in curl tar git openssh rclone coreutils proot-distro; do
    if ! need "$pk"; then
      pkg install -y "$pk" >/dev/null 2>&1 || warn "could not install $pk — continuing"
    fi
  done
  # The release download happens inside the Debian container, so Termux
  # itself needs no curl for the install — only proot-distro is required.
  need proot-distro || die "proot-distro is required — run 'pkg install proot-distro' and re-run."
elif [[ "$OS" == Linux ]] && need apt-get; then
  say "Installing light prerequisites (curl, tar)…"
  SU=""
  need sudo && SU="sudo"
  ${SU} apt-get update -qq >/dev/null 2>&1 || true
  ${SU} apt-get install -y -qq curl tar >/dev/null 2>&1 || warn "apt hiccup; continuing"
fi
if [[ "$TERMUX" != 1 ]]; then
  need curl || die "curl is required (install it and re-run)"
  need tar  || die "tar is required (install it and re-run)"
  need sha256sum || warn "sha256sum missing — checksum verify will be skipped"
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
      proot-distro remove debian --force >/dev/null 2>&1 \
        && ok "previous 'debian' instance removed" \
        || warn "could not remove it — run 'proot-distro remove debian --force' once."
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
    printf 'REPO="%s"\nBIN_NAME="%s"\n' "$REPO" "$BIN_NAME"
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

# 3) LAST — the latest DeCypherTek.ai release, downloaded and installed
#    here inside the Linux container (not in Termux).
say "Debian: downloading the latest DeCypherTek.ai release (last step)…"
GH_API="https://api.github.com/repos/$REPO/releases"
if ! RELEASE_JSON="$(curl -sSLf "${AUTH[@]}" "$GH_API/latest" 2>/dev/null)"; then
  die "No releases yet. Trigger the Release workflow (Actions → Release → Run workflow) or build with: cargo install --path ."
fi
TAG="$(printf '%s' "$RELEASE_JSON" | grep -o '"tag_name": *"[^"]*"' | head -1 | sed 's/.*"tag_name": *"//; s/"$//')"
if [[ -z "$TAG" ]]; then
  die "No releases yet. Trigger the Release workflow (Actions → Release → Run workflow) or build with: cargo install --path ."
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

# ---------------------------------------------------------------- docker (desktop)
setup_docker() {
  if [[ "$TERMUX" == 1 ]]; then
    return  # handled inside the proot Debian above
  fi
  if need docker; then
    ok "Docker already installed: $(docker --version 2>/dev/null || echo present)"
    return
  fi
  say "Best-effort Docker setup (optional — @store's MCP servers need it)…"
  SU=""
  need sudo && SU="sudo"
  if need apt-get; then
    ${SU} apt-get install -y -qq docker.io docker-compose >/dev/null 2>&1 \
      && ok "Docker installed (docker.io docker-compose)" \
      || warn "Docker install failed — install manually for @store containers."
  elif need dnf; then
    ${SU} dnf install -y docker >/dev/null 2>&1 \
      && ok "Docker installed" \
      || warn "Docker install failed — install manually for @store containers."
  else
    warn "No supported package manager — install Docker manually if you want it."
  fi
}

# ------------------------------------------------------------ latest release
# Desktop path: fetch + verify + stage the latest release binary. On
# Termux this whole step happens INSIDE the Debian container instead
# (see the bootstrap above) — nothing is downloaded into Termux.
fetch_release() {
  say "Finding the latest release…"
  GH_API="https://api.github.com/repos/$REPO/releases"
  AUTH=()
  if [[ -n "${GH_TOKEN:-}" || -n "${GITHUB_TOKEN:-}" ]]; then
    AUTH=(-H "Authorization: Bearer ${GH_TOKEN:-$GITHUB_TOKEN}")
  fi
  if ! RELEASE_JSON="$(curl -sSLf "${AUTH[@]}" "$GH_API/latest" 2>/dev/null)"; then
    die "No releases yet. Trigger the Release workflow (Actions → Release → Run workflow) or build with: cargo install --path ."
  fi

  TAG="$(printf '%s' "$RELEASE_JSON" | grep -o '"tag_name": *"[^"]*"' | head -1 | sed 's/.*"tag_name": *"//; s/"$//')"
  if [[ -z "$TAG" ]]; then
    die "No releases yet. Trigger the Release workflow (Actions → Release → Run workflow) or build with: cargo install --path ."
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

  if [[ -f "$TMPD/checksums.txt" ]] && need sha256sum; then
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
  BIN_STAGED="$TMPD/unpack/decyphertek"
  chmod +x "$BIN_STAGED"
  INSTALLED_VERSION="$("$BIN_STAGED" --version 2>/dev/null || echo unknown)"
}

# ---------------------------------------------------------------- install
if [[ "$TERMUX" == 1 ]]; then
  termux_proot_home
  LAUNCHER="$PREFIX/bin/$BIN_NAME"
  ok "DeCypherTek.ai installed inside proot Debian — '$BIN_NAME' launches it from Termux"
else
  fetch_release
  DEST_BIN="$HOME/.local/bin"
  mkdir -p "$DEST_BIN"
  mv "$BIN_STAGED" "$DEST_BIN/$BIN_NAME"
  chmod +x "$DEST_BIN/$BIN_NAME"
  LAUNCHER="$DEST_BIN/$BIN_NAME"
  setup_docker
  echo
  if [[ ":$PATH:" != *":$DEST_BIN:"* ]]; then
    warn "$DEST_BIN is not on PATH — adding it to your shell profile."
    for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
      if [[ -f "$rc" ]] && ! grep -q '\.local/bin' "$rc"; then
        printf '\nexport PATH="$HOME/.local/bin:$PATH"\n' >> "$rc"
      fi
    done
    export PATH="$DEST_BIN:$PATH"
  fi
  ok "DeCypherTek.ai on PATH: $INSTALLED_VERSION"
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
