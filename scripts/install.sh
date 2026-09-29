#!/usr/bin/env bash
# DeCypherTek.ai — one-command installer.
#
# Termux first, and the ORDER matters: nothing of the agent is
# downloaded into Termux itself. The installer bootstraps a proot
# Debian Linux home first, and every real step happens INSIDE that
# Linux container, in this order:
#   1. apt update
#   2. apt install -y docker.io docker-compose curl gnupg ca-certificates
#   3. LAST: download the latest DeCypherTek.ai release and install it
#      as /usr/local/bin/decyphertek.ai inside Debian
#
# Docker does not work in Termux proper (no root), so that proot Debian
# is where the daemon lives (best-effort start included). Back in
# Termux, typing `decyphertek.ai` runs a SOURCED ALIAS (installed into
# ~/.bashrc — plus the launcher script it points to) that drops you
# straight into the proot Debian and launches the agent there.
# You get a regular terminal that passes through everything typed
# except @-commands — @chat / @code / @research / @store wake the agent.
# MCP tool servers pulled via @store run in hardened containers that
# talk only to the agent (network=none, cap-drop=ALL, stdio-only).
#
# On Linux/macOS: same single binary on PATH as `decyphertek.ai`, plus a
# best-effort Docker setup so @store works there too.
#
# No separate setup step either way: the installer ends by launching
# decyphertek.ai itself, the launcher detects a fresh install, runs the
# walkthrough (persona, OpenRouter/Ollama, the Leash, @store servers are
# optional adds), and drops you in the @-shell after.
# Re-run any time to update to the latest release.
set -euo pipefail

REPO="decyphertek-io/DeCypherTek.ai"
BIN_NAME="decyphertek.ai"
DATA_DIR="${HOME}/.decyphertek.ai"
PD_ROOT=""    # set on Termux: debian rootfs location
PROOT_DISTRO="debian"

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

# =========================================== TERMUX: proot Debian + Docker
# Docker cannot run in Termux itself (no root, no cgroups). The agent's
# home becomes a proot Debian Linux where Docker CAN be installed, and
# the daemon is started with bridge/iptables off (proot-safe mode).
# @store's MCP servers then run in hardened containers — network=none
# anyway.
#
# proot-distro v5+ pulls OCI images and keeps the rootfs under
# containers/<name>/rootfs; v4 used plugin tarballs under
# installed-rootfs/<name>. Support both layouts so fresh installs and
# proot-distro upgrades keep working. Debian is first-class in
# proot-distro on every architecture (aarch64, armhf, x86_64), so one
# plain `proot-distro install debian` covers every device.

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

pd_install_debian() {
  say "Installing the Debian rootfs image…"
  # Keep the local container name "debian" for the rest of the flow.
  proot-distro install "$PROOT_DISTRO" >/dev/null 2>&1
}

# The Debian-side bootstrap script. Written into the rootfs and run
# INSIDE the container, strictly in this order: apt update first, then
# the Docker toolchain, and the DeCypherTek.ai release download LAST —
# the agent binary is never staged in Termux.
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

# 2) The Docker toolchain + TLS certs + https tools inside Debian.
say "Debian: apt install docker.io docker-compose curl gnupg ca-certificates…"
PKGS=(docker.io docker-compose curl gnupg ca-certificates)
if ! apt-get install -y -qq "${PKGS[@]}" >/dev/null 2>&1; then
  warn "apt could not install the whole set at once — trying package by package…"
  for pk in "${PKGS[@]}"; do
    apt-get install -y -qq "$pk" >/dev/null 2>&1 || warn "could not install $pk — continuing"
  done
fi
command -v curl >/dev/null 2>&1 || die "curl is required inside Debian — re-run the installer."
ok "Debian: docker.io docker-compose curl gnupg ca-certificates ready"

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
  say "Docker — and therefore MCP tool servers from @store — can live)…"

  if ! pd_locate_rootfs; then
    pd_install_debian \
      || die "proot-distro could not install Debian — run 'pkg upgrade' and re-run."
    pd_locate_rootfs \
      || die "debian rootfs not found after install — re-run to retry."
    ok "debian linux rootfs installed"
  else
    ok "debian linux rootfs already present"
  fi

  # Everything below runs inside the Debian container, in order — and
  # the DeCypherTek.ai download is the LAST step.
  dct_write_bootstrap
  say "Inside Debian: apt update, then dockerd's toolchain (docker.io"
  say "docker-compose curl gnupg ca-certificates), then the release download…"
  if ! proot-distro login "$PROOT_DISTRO" -- bash /root/dct-bootstrap.sh; then
    rm -f "$PD_ROOT/root/dct-bootstrap.sh"
    die "the Debian bootstrap failed — run 'pkg upgrade' and re-run the installer."
  fi
  rm -f "$PD_ROOT/root/dct-bootstrap.sh"
  ok "binary deployed inside debian: /usr/local/bin/$BIN_NAME"

  # Launcher inside debian: start dockerd if needed (proot-safe flags),
  # then exec the agent. Setup detection is the agent's own job. A flag
  # file (in the bind-mounted vault dir) marks kernels that refused the
  # daemon, so later launches don't wait on a dead dockerd.
  mkdir -p "$PD_ROOT/usr/local/bin"
  cat > "$PD_ROOT/usr/local/bin/dct-launch" <<'LAUNCH'
#!/bin/bash
# DeCypherTek on proot Debian: dockerd best-effort + straight into the agent.
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
exec /usr/local/bin/decyphertek.ai "$@"
LAUNCH
  chmod +x "$PD_ROOT/usr/local/bin/dct-launch"

  # Try the daemon once, inside ONE proot session (a daemon dies with its
  # session — so start + probe must share the login).
  say "Probing the Docker daemon inside proot Debian (first boot may take a moment)…"
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

  # Termux side: the launcher script on $PREFIX/bin (works everywhere),
  # plus the SOURCED ALIAS the user actually types — `decyphertek.ai`
  # goes from a Termux prompt straight into the proot Debian.
  mkdir -p "$PREFIX/bin"
  cat > "$PREFIX/bin/$BIN_NAME" <<WRAP
#!/data/data/com.termux/files/usr/bin/bash
# DeCypherTek.ai — from Termux straight into the proot Debian home.
# The vault data stays in real Termux home and is bind-mounted in
# (survives container rebuilds); the agent + Docker live inside Debian.
DATA="\$HOME/.decyphertek.ai"
mkdir -p "\$DATA"
ARGS=(login $PROOT_DISTRO --bind "\$DATA:/root/.decyphertek.ai")
[[ -d /sdcard ]] && ARGS+=(--bind /sdcard:/sdcard)
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
  ok "no existing agent — the walkthrough (persona, brain, leash, vault"
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
