#!/usr/bin/env bash
# DeCypherTek.ai — one-command installer.
#
# Termux first: on Android the agent gets a proot Arch Linux home. Docker
# does not work in Termux proper (no root), so the installer bootstraps
# `proot-distro` Arch Linux, installs the decyphertek binary AND Docker
# inside it (best-effort daemon start included), and the `decyphertek`
# command becomes a wrapper that drops you straight into the proot Arch.
# There you get a regular terminal that passes through everything typed
# except @-commands — @chat / @code / @research / @store wake the agent.
# MCP tool servers pulled via @store run in hardened containers that
# talk only to the agent (network=none, cap-drop=ALL, stdio-only).
#
# On Linux/macOS: same single binary on PATH, plus a best-effort Docker
# setup so @store works there too.
#
# No separate setup step either way: the launcher detects a fresh
# install, runs the walkthrough (persona, OpenRouter/Ollama, the Leash,
# @store servers are optional adds), and drops you in the @-shell after.
# Re-run any time to update to the latest release.
set -euo pipefail

REPO="decyphertek-io/DeCypherTek.ai"
BIN_NAME="decyphertek"
DATA_DIR="${HOME}/.decyphertek.ai"
PD_ROOT=""    # set on Termux: arch rootfs location
PROOT_DISTRO="archlinux"

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
elif [[ "$OS" == Linux ]] && need apt-get; then
  say "Installing light prerequisites (curl, tar)…"
  SU=""
  need sudo && SU="sudo"
  ${SU} apt-get update -qq >/dev/null 2>&1 || true
  ${SU} apt-get install -y -qq curl tar >/dev/null 2>&1 || warn "apt hiccup; continuing"
fi
need curl || die "curl is required (install it and re-run)"
need tar  || die "tar is required (install it and re-run)"
need sha256sum || warn "sha256sum missing — checksum verify will be skipped"
echo

# ============================================== TERMUX: proot Arch + Docker
# Docker cannot run in Termux itself (no root, no cgroups). The agent's
# home becomes a proot Arch Linux where Docker CAN be installed, and the
# daemon is started with bridge/iptables off (proot-safe mode). @store's
# MCP servers then run in hardened containers — network=none anyway.
termux_proot_home() {
  PD_ROOT="${PREFIX}/var/lib/proot-distro/installed-rootfs/${PROOT_DISTRO}"

  say "Setting up the proot Arch Linux home for the agent (this is where"
  say "Docker — and therefore MCP tool servers from @store — can live)…"

  if [[ ! -d "$PD_ROOT" ]]; then
    proot-distro install "$PROOT_DISTRO" >/dev/null 2>&1 \
      || die "proot-distro install $PROOT_DISTRO failed — re-run to retry."
    ok "arch linux rootfs installed"
  else
    ok "arch linux rootfs already present"
  fi

  # Binary + launcher live INSIDE the rootfs; the vault data stays in
  # real Termux home and is bind-mounted in (survives container rebuilds).
  mkdir -p "$PD_ROOT/usr/local/bin"
  cp "$BIN_STAGED" "$PD_ROOT/usr/local/bin/$BIN_NAME"
  chmod +x "$PD_ROOT/usr/local/bin/$BIN_NAME"
  ok "binary deployed inside arch: /usr/local/bin/$BIN_NAME"

  # Launcher inside arch: start dockerd if needed (proot-safe flags),
  # then exec the agent. Setup detection is the agent's own job. A flag
  # file (in the bind-mounted vault dir) marks kernels that refused the
  # daemon, so later launches don't wait on a dead dockerd.
  cat > "$PD_ROOT/usr/local/bin/dct-launch" <<'LAUNCH'
#!/bin/bash
# DeCypherTek on proot Arch: dockerd best-effort + straight into the agent.
DATA=/root/.decyphertek.ai
mkdir -p "$DATA" 2>/dev/null || DATA=/root
if command -v docker >/dev/null 2>&1; then
  if timeout 5 docker info >/dev/null 2>&1; then
    rm -f "$DATA/.dockerd-broken" 2>/dev/null || true
  elif [[ ! -f "$DATA/.dockerd-broken" ]]; then
    (dockerd --iptables=false --bridge=none >"$DATA/dockerd.log" 2>&1 &)
    for _ in $(seq 1 15); do
      timeout 5 docker info >/dev/null 2>&1 && break
      sleep 1
    done
    if ! timeout 5 docker info >/dev/null 2>&1; then
      touch "$DATA/.dockerd-broken" 2>/dev/null || true
      echo "note: dockerd did not start on this device/kernel — @store will report it; log: $DATA/dockerd.log"
    fi
  fi
fi
exec /usr/local/bin/decyphertek "$@"
LAUNCH
  chmod +x "$PD_ROOT/usr/local/bin/dct-launch"

  # Termux-side wrapper: `decyphertek` = enter the proot arch, run agent.
  # Bind the vault data dir + shared storage so grants/memories are real.
  cat > "$PREFIX/bin/$BIN_NAME" <<WRAP
#!/data/data/com.termux/files/usr/bin/bash
# DeCypherTek.ai — @-shell inside the proot Arch (Docker home of @store).
DATA="\$HOME/.decyphertek.ai"
mkdir -p "\$DATA"
ARGS=(login "$PROOT_DISTRO" --bind "\$DATA:/root/.decyphertek.ai")
[[ -d /sdcard ]] && ARGS+=(--bind /sdcard:/sdcard)
proot-distro "\${ARGS[@]}" -- /usr/local/bin/dct-launch "\$@"
WRAP
  chmod +x "$PREFIX/bin/$BIN_NAME"
  ok "wrapper installed: $PREFIX/bin/$BIN_NAME (enters proot arch + agent)"

  # Docker inside the arch rootfs — best effort, auto-start on launch.
  if [[ -x "$PD_ROOT/usr/bin/pacman" ]]; then
    say "Installing Docker inside proot Arch (best effort; on-device kernels vary)…"
    proot-distro login "$PROOT_DISTRO" -- pacman -Sy --noconfirm --needed docker \
      >/dev/null 2>&1 \
      && ok "docker installed inside arch" \
      || warn "pacman could not install docker — @store will report details."
  else
    warn "pacman not found in rootfs — run proot-distro reinstall $PROOT_DISTRO"
  fi

  # Try the daemon once, inside ONE proot session (a daemon dies with its
  # session — so start + probe must share the login).
  say "Probing the Docker daemon inside proot Arch (first boot may take a moment)…"
  PROBE="$(proot-distro login "$PROOT_DISTRO" -- bash -c \
    '/usr/local/bin/dct-launch --version >/dev/null 2>&1; \
     if timeout 5 docker info >/dev/null 2>&1; then echo ok; else echo off; fi' \
    2>/dev/null || echo off)"
  if [[ "$PROBE" == ok ]]; then
    ok "dockerd answered inside proot — @store MCP servers fully operational."
  else
    warn "dockerd did not come up on this device/kernel. The agent runs fine;"
    warn "@store lists and registers servers; launching them needs a daemon —"
    warn "either this same flow on a device where proot dockerd works, or a"
    warn "DOCKER_HOST pointed at a LAN machine. Log: /root/.decyphertek.ai/dockerd.log"
  fi
  echo
}

# ---------------------------------------------------------------- docker (desktop)
setup_docker() {
  if [[ "$TERMUX" == 1 ]]; then
    return  # handled inside the proot arch above
  fi
  if need docker; then
    ok "Docker already installed: $(docker --version 2>/dev/null || echo present)"
    return
  fi
  say "Best-effort Docker setup (optional — @store's MCP servers need it)…"
  SU=""
  need sudo && SU="sudo"
  if need apt-get; then
    ${SU} apt-get install -y -qq docker.io >/dev/null 2>&1 \
      && ok "Docker installed (docker.io)" \
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
if [[ ! -f "$TMPD/unpack/$BIN_NAME" ]]; then
  die "asset did not contain the '$BIN_NAME' binary"
fi
BIN_STAGED="$TMPD/unpack/$BIN_NAME"
chmod +x "$BIN_STAGED"
INSTALLED_VERSION="$("$BIN_STAGED" --version 2>/dev/null || echo unknown)"

# ---------------------------------------------------------------- install
if [[ "$TERMUX" == 1 ]]; then
  termux_proot_home
  LAUNCHER="$PREFIX/bin/$BIN_NAME"
else
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
fi
ok "DeCypherTek on PATH: $INSTALLED_VERSION"
echo

# ---------------------------------------------------------------- launch
# No separate setup step: the agent detects whether it has been set up,
# runs the walkthrough itself if not, then lands in the @-shell — a
# regular terminal that passes through everything except @-commands.
# @setup re-runs the walkthrough any time while inside.
if [[ -f "$DATA_DIR/vault.dct" ]]; then
  ok "existing agent found at $DATA_DIR — update complete."
else
  ok "no existing agent — the walkthrough (persona, brain, leash, vault"
  ok "password) starts automatically on first launch. @setup re-runs it."
fi

if [[ -t 0 ]]; then
  say "Launching DeCypherTek.ai…"
  exec "$LAUNCHER"
else
  say "Not a terminal — start it yourself with: $BIN_NAME"
fi
