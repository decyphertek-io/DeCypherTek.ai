#!/usr/bin/env bash
# DeCypherTek.ai — one-command installer. Linux only: aarch64 or x86_64.
#
# Two homes, one command — the installer DETECTS which machine it is
# on and installs the right release for it, into one folder both
# times: ~/.decyphertek.ai (program + encrypted vault data together).
#
# PODROID — the Android app that boots a REAL Alpine Linux VM with its
# own kernel (QEMU/AVF; a virtual machine, not a proot translation
# layer). Detected by fingerprints: /etc/podroid, its OpenRC services
# (/etc/init.d/podroid-*), the podroid-hostd bridge daemon, the
# podroid.* kernel cmdline markers. The guest already IS a Linux
# machine: nothing is bootstrapped, the plain one-folder install runs
# directly inside the VM. Podman ships pre-installed and rootless-ready
# inside that Alpine VM, and the MCP tool servers @store launches
# connect to exactly that podman: the agent keeps issuing plain
# `docker` commands, so the installer writes a small docker->podman
# shim into the agent bin dir it already prepends to PATH —
# ~/.decyphertek.ai/bin/docker, found purely by PATH order. The VM's
# own real docker package (/usr/bin/docker + dockerd) stays untouched,
# runnable by full path exactly as Podroid shipped it. Alpine's default
# shell is busybox ash (a login shell that reads ~/.profile, never
# ~/.bashrc), so the PATH block additionally lands in ~/.profile —
# there and on any other Alpine.
#
# PC/BOX LINUX — Debian, Ubuntu, Fedora, Arch, openSUSE, Alpine … on
# x86_64 or aarch64: the release binary lands in
# ~/.decyphertek.ai/bin/ — the same ~/.decyphertek.ai that already
# holds vault.dct and staging/, so the whole agent (program + encrypted
# data) is a single directory. A marked, removable PATH block in
# ~/.bashrc / ~/.zshrc makes `decyphertek.ai` launchable from anywhere.
# What the installer DOES along the way is picked by detection gates:
# which Linux flavor (DISTRO_ID — Debian, Ubuntu, Fedora, Arch,
# openSUSE, Alpine …), which package manager (PKG_MGR — apt-get / dnf /
# pacman / zypper / apk), which CPU (ARCH -> the release asset).
# @store's plain `docker` commands are served best-effort by daemonless
# podman + the podman-docker shim — the same runtime the agent uses
# inside the Podroid VM, so there is no docker daemon to enable, start
# or babysit anywhere.
#
# Platforms this installer refuses, on purpose — one command, and it
# says so instead of half-working:
#   * Termux (Android, no VM): no real kernel, no cgroups, no
#     containers. On Android the agent runs inside Podroid's Linux VM
#     — the same one-command install runs there.
#   * macOS, Windows and every other OS: Linux only, aarch64 or x86_64.
#   * 32-bit arms and exotic CPUs: aarch64 (arm64) or x86_64 only.
#
# No separate setup step either way: the installer ends by launching
# decyphertek.ai itself, the launcher detects a fresh install and runs
# a SHORT first-run wizard — just the brain (OpenRouter key + model, or
# Ollama) and the Leash. Everything else defaults, and every default
# path lives in the ONE folder ~/.decyphertek.ai (vault, llm wiki,
# research profiles, RAG memory) — then it drops you in the @-shell.
# Customizing is one command away: /setup re-runs the full wizard
# (memory folders, grants, research profiles) any time.
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

say()  { printf '\033[36m>>\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m ✓\033[0m %s\n' "$*"; }
warn() { printf '\033[33m !\033[0m %s\n' "$*"; }
die()  { printf '\033[31m x\033[0m %s\n' "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1; }

case "$(uname -s)" in
  MSYS*|MINGW*|CYGWIN*) die "Windows is not supported — Linux only (aarch64 or x86_64)." ;;
  Darwin) die "macOS is not supported — Linux only (aarch64 or x86_64)." ;;
  Linux*) ;;
  *) die "Unsupported OS: $(uname -s) — Linux only (aarch64 or x86_64), on the bare metal or in a VM (Podroid on Android)." ;;
esac

# Android without a VM does not work: the agent and its @store
# containers need a real kernel. Refuse fast, point at Podroid.
if [[ -n "${TERMUX_VERSION:-}" || "${PREFIX:-}" == *com.termux* ]]; then
  die "Termux is not supported — a translation layer is no kernel: no cgroups, \
      no containers. On Android, run this same command inside Podroid's Linux \
      VM (or the Mobian VM app). Everywhere else: any Linux aarch64/x86_64 terminal."
fi

# ---------------------------------------------------------------- platform
# Boolean logic gates: what the installer runs on decides what it does.
#   PODROID=1 — Android/Podroid: the terminal is already inside a REAL
#               Alpine Linux VM with podman pre-installed, so the plain
#               one-folder install runs in the VM, and @store's `docker`
#               commands are served by that pre-installed podman.
#   PC Linux  — the flavor (DISTRO_ID via /etc/os-release) and its
#               package manager (PKG_MGR) pick the container-runtime
#               package set; the install itself is ONE folder,
#               ~/.decyphertek.ai, everywhere.
ARCH="$(uname -m)"

# Podroid detection: everything the guest bakes into its Alpine squashfs
# is a fingerprint — the /etc/podroid config dir, its OpenRC services
# (/etc/init.d/podroid-*), the podroid-hostd guest->Android bridge
# daemon, or the podroid.* markers the app passes on the kernel command
# line. Any single one means the installer is running INSIDE the VM, a
# real Linux machine, so the PC path below applies with one special:
# MCP containers connect to the VM's pre-installed podman.
PODROID=0
if [[ -d /etc/podroid ]] \
   || [[ -x /usr/local/bin/podroid-hostd ]] \
   || grep -q 'podroid\.' /proc/cmdline 2>/dev/null \
   || compgen -G '/etc/init.d/podroid-*' >/dev/null 2>&1; then
  PODROID=1
fi

# The platforms we ship: aarch64 (arm64) and x86_64, static musl.
case "$ARCH" in
  aarch64|arm64) TARGET="aarch64-unknown-linux-musl" ;;
  x86_64)        TARGET="x86_64-unknown-linux-musl" ;;
  *) die "Unsupported CPU: $ARCH — Linux aarch64 (arm64) or x86_64 only. Build from source with: cargo install --path ." ;;
esac

# Which Linux flavor: this is the diff between an apt, a dnf, a pacman,
# a zypper and an apk install further down.
DISTRO_ID=""
DISTRO_PRETTY=""
if [[ -r /etc/os-release ]]; then
  DISTRO_ID="$(sed -n 's/^ID=//p' /etc/os-release | head -1 | tr -d '"')"
  DISTRO_PRETTY="$(sed -n 's/^PRETTY_NAME=//p' /etc/os-release | head -1 | tr -d '"')"
fi

# The package-manager gate drives every package install from here on
# (prerequisites and the container runtime), so Debian vs Fedora vs Arch
# each take their own command.
PKG_MGR=""
for _m in apt-get dnf yum pacman zypper apk; do
  if need "$_m"; then
    PKG_MGR="$_m"
    break
  fi
done

if [[ "$PODROID" == 1 ]]; then
  say "Detected: Podroid — Alpine Linux VM on Android ($ARCH) -> $TARGET"
  say "Install mode: one folder — $DATA_DIR; @store MCP containers on the"
  say "VM's pre-installed podman (the VM is already real Linux)."
else
  say "Detected: ${DISTRO_PRETTY:-Linux} ($ARCH) -> $TARGET"
  say "Install mode: one folder — $DATA_DIR; runtime packages via ${PKG_MGR:-no package manager detected}."
fi

# ----------------------------------------------------------------- channel
# Which build to install — asked exactly once, prominently:
#   production   — releases/latest: the stable Prod-Build output every
#                  machine gets by default.
#   experimental — the dev prerelease the 'dev-adminotaur' testing
#                  branch publishes (its single rolling
#                  v<version>-dev-adminotaur tag — every push
#                  overwrites the previous build in place, and it can
#                  never be confused with the usaginotsuki channel).
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
# apt, Fedora's dnf, Arch's pacman, openSUSE's zypper, Alpine's apk —
# so the same installer is portable across flavors. As root — the
# Podroid VM's default root login, a root server shell — no elevation
# is attempted at all: sudo is only prepended for non-root users that
# have it.
SU=""
if [[ "$(id -u)" != 0 ]] && need sudo; then SU="sudo"; fi

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
    *)      return 1 ;;
  esac
}

if [[ -n "$PKG_MGR" ]] && { ! need curl || ! need tar; }; then
  say "Installing prerequisites (curl, tar) via $PKG_MGR…"
  pkg_install curl tar || warn "could not install prerequisites — continuing"
fi
need curl || die "curl is required (install it and re-run)"
need tar  || die "tar is required (install it and re-run)"
SUMTOOL=""
if need sha256sum; then
  SUMTOOL="sha256sum"
else
  warn "no sha256sum found — checksum verify will be skipped"
fi
echo

# ------------------------------------------------ container runtime (Linux)
# @store pulls, runs and manages MCP tool servers with plain `docker`
# commands. On a Linux box that CLI comes best-effort from daemonless
# podman + the podman-docker shim — the same runtime the agent uses
# inside the Podroid VM — so no docker daemon needs enabling, starting
# or babysitting anywhere. The package set is chosen by the detected
# flavor: Debian/Ubuntu (apt) also gets uidmap so rootless podman can
# map subordinate UIDs right away; Fedora (dnf), Arch (pacman), openSUSE
# (zypper), Alpine (apk) ship those defaults.
setup_container_runtime() {
  if [[ "$PODROID" == 1 ]]; then
    # Podroid: the Alpine VM ships podman pre-installed and rootless-ready
    # (crun, fuse-overlayfs, netavark, setcap'd newuidmap baked into its
    # squashfs) — @store's MCP servers connect to exactly that podman, so
    # nothing is installed over it. But @store keeps issuing plain `docker`
    # commands (info / images / pull / run), and the VM's real docker
    # package owns /usr/bin/docker (apk-installing Alpine's podman-docker
    # would just fight it), so the docker->podman shim lands in the agent
    # bin dir this installer already prepends to PATH: `docker` resolves to
    # podman by PATH order, and Podroid's own docker (dockerd included)
    # stays exactly as the VM shipped it — runnable by full path.
    if ! need podman; then
      warn "podman missing inside this Podroid VM (unexpected) — trying apk…"
      pkg_install podman || warn "could not install podman — @store browsing works, MCP launches will not."
    fi
    if need podman; then
      mkdir -p "$DEST_BIN"
      cat > "$DEST_BIN/docker" <<'PODROIDSHIM'
#!/bin/sh
# DeCypherTek.ai — the `docker` CLI, served by podman on Podroid.
# Dropped here by scripts/install.sh: the VM's pre-installed daemonless
# podman is what @store's MCP tool containers connect to, and this shim
# routes the agent's plain `docker` commands to it. Found purely by PATH
# order; uninstalling the agent (scripts/uninstall.sh) removes it.
exec podman "$@"
PODROIDSHIM
      chmod +x "$DEST_BIN/docker"
      ok "docker -> podman shim wired: $DEST_BIN/docker — @store MCP"
      ok "containers run on the VM's pre-installed podman; Podroid's own"
      ok "docker (/usr/bin/docker, dockerd) is untouched."
      say "Waking podman up once (first run in a fresh VM can take a moment)…"
      PROBE="off"
      if need timeout; then
        if timeout 30 podman info >/dev/null 2>&1; then PROBE="ok"; fi
      else
        if podman info >/dev/null 2>&1; then PROBE="ok"; fi
      fi
      if [[ "$PROBE" == "ok" ]]; then
        ok "podman answered inside Podroid — @store fully operational."
      else
        warn "podman did not answer within 30 s — the agent itself runs fine;"
        warn "@store registers servers, and launches usually recover once"
        warn "the VM has been up a moment. Re-run the installer to re-probe."
      fi
    else
      warn "no podman inside this Podroid VM — @store MCP launches disabled."
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
# Fetch + verify + stage the chosen channel's release — production ->
# releases/latest, experimental -> the branch-labeled v*-dev-adminotaur
# prerelease published from the testing branch. The release assets are
# static musl builds: aarch64 (arm64) and x86_64 only.
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
    # dev-adminotaur's branch-labeled tag (never usaginotsuki's);
    # interim/legacy -dev / -dev.<sha> tags kept as a fallback.
    TAG="$(printf '%s' "$RELIST" | grep -o '"tag_name": *"[^"]*-dev-adminotaur"' | head -1 | sed 's/.*"tag_name": *"//; s/"$//')"
    if [[ -z "$TAG" ]]; then
      TAG="$(printf '%s' "$RELIST" | grep -o '"tag_name": *"[^"]*-dev[0-9a-f.]*"' | head -1 | sed 's/.*"tag_name": *"//; s/"$//')"
    fi
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
# ONE folder everywhere: the binary lives in ~/.decyphertek.ai/bin — the
# same ~/.decyphertek.ai that will hold vault.dct and staging/, so
# program + encrypted data is a single directory: an uninstall without
# --purge removes bin/ and keeps the vault; --purge removes everything.
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

# PATH: a marked, removable block — written only when missing. On
# Podroid and any other Alpine the default shell is busybox ash: a
# LOGIN shell that reads ~/.profile and never ~/.bashrc, so the block
# additionally goes to ~/.profile there.
if [[ ":$PATH:" != *":$DEST_BIN:"* ]]; then
  ASH_PROFILE=0
  if [[ "$PODROID" == 1 || "$DISTRO_ID" == "alpine" ]]; then ASH_PROFILE=1; fi
  for rc in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
    if [[ "$rc" == "$HOME/.zshrc" && ! -f "$rc" ]]; then continue; fi
    if [[ "$rc" == "$HOME/.profile" && "$ASH_PROFILE" != 1 ]]; then continue; fi
    touch "$rc"
    if ! grep -qF '.decyphertek.ai/bin' "$rc"; then
      printf '\n# DeCypherTek.ai — agent bin on PATH (uninstall.sh removes this block)\nexport PATH="$HOME/.decyphertek.ai/bin:$PATH"\n' >> "$rc"
    fi
  done
  export PATH="$DEST_BIN:$PATH"
  ok "PATH wired: $DEST_BIN"
fi
ok "DeCypherTek.ai $TAG ($CHANNEL channel, $INSTALLED_VERSION) — everything under $DATA_DIR"
echo

# ---------------------------------------------------------------- launch
# No separate setup step: the agent detects whether it has been set up,
# runs the walkthrough itself if not, then lands in the @-shell — a
# regular terminal that passes through everything except @-commands.
# /setup re-runs the walkthrough any time while inside.
if [[ -f "$DATA_DIR/vault.dct" ]]; then
  ok "existing agent found at $DATA_DIR — update complete."
else
  ok "no existing agent — first launch asks just the brain (OpenRouter"
  ok "key + model, or Ollama) and the Leash; everything else defaults"
  ok "into $DATA_DIR (llm wiki, research profiles, RAG memory)."
  ok "to customize, type /setup"
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
