#!/usr/bin/env bash
# DeCypherTek.ai — one-command installer.
#
# Works on Termux (Android), Linux, and macOS. Termux first: if it works
# here, it works anywhere. Detects the CPU, downloads the latest release
# binary from GitHub Releases (verifies the SHA-256), installs it on PATH,
# does a best-effort Docker setup where Docker is actually possible, then
# launches the TUI walkthrough wizard (persona, OpenRouter/Ollama, memory
# folders, the leash, the vault password).
#
# Re-run any time to update to the latest release.
set -euo pipefail

REPO="decyphertek-io/DeCypherTek.ai"
BIN_NAME="decyphertek"
DATA_DIR="${HOME}/.decyphertek.ai"

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
  for pk in curl tar git openssh rclone coreutils; do
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

# ---------------------------------------------------------------- docker
setup_docker() {
  if [[ "$TERMUX" == 1 ]]; then
    warn "Docker does not run natively on Android (no root) — skipping."
    say "MCP tool servers run as local processes in Termux. The built-in"
    say "tools run natively already; for remote Docker later, point"
    say "DOCKER_HOST at a desktop over SSH."
    return
  fi
  if need docker; then
    ok "Docker already installed: $(docker --version 2>/dev/null || echo present)"
    return
  fi
  say "Best-effort Docker setup (optional — the agent needs no containers)…"
  SU=""
  need sudo && SU="sudo"
  if need apt-get; then
    ${SU} apt-get install -y -qq docker.io >/dev/null 2>&1 \
      && ok "Docker installed (docker.io)" \
      || warn "Docker install failed — install manually if you want containers."
  elif need dnf; then
    ${SU} dnf install -y docker >/dev/null 2>&1 \
      && ok "Docker installed" \
      || warn "Docker install failed — install manually if you want containers."
  else
    warn "No supported package manager — install Docker manually if you want it."
  fi
}
setup_docker
echo

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

# ---------------------------------------------------------------- install
if [[ "$TERMUX" == 1 ]]; then
  DEST_BIN="$PREFIX/bin"
else
  DEST_BIN="$HOME/.local/bin"
  mkdir -p "$DEST_BIN"
fi
mv "$TMPD/unpack/$BIN_NAME" "$DEST_BIN/$BIN_NAME"
chmod +x "$DEST_BIN/$BIN_NAME"
ok "installed: $DEST_BIN/$BIN_NAME"

if [[ ":$PATH:" != *":$DEST_BIN:"* ]]; then
  warn "$DEST_BIN is not on PATH — adding it to your shell profile."
  for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
    if [[ -f "$rc" ]] && ! grep -q '\.local/bin' "$rc"; then
      printf '\nexport PATH="$HOME/.local/bin:$PATH"\n' >> "$rc"
    fi
  done
  export PATH="$DEST_BIN:$PATH"
fi

INSTALLED_VERSION="$("$DEST_BIN/$BIN_NAME" --version 2>/dev/null || echo unknown)"
ok "DeCypherTek on PATH: $INSTALLED_VERSION"
echo

# ---------------------------------------------------------------- wizard
if [[ -f "$DATA_DIR/vault.dct" ]]; then
  ok "existing agent found at $DATA_DIR — update complete."
  if [[ -t 0 ]]; then
    say "Reconfigure now (walkthrough: persona, brain, folders, leash)?"
    read -r -p "[$BIN_NAME setup] y/N: " answer </dev/tty || answer=n
    case "$answer" in
      y|Y*)
        "$DEST_BIN/$BIN_NAME" setup && ok "reconfigured + re-sealed" \
          || warn "wizard did not finish — memory unchanged" ;;
      *) say "Keeping current configuration. Unlock with: $BIN_NAME" ;;
    esac
  fi
elif [[ -t 0 ]]; then
  say "Launching the first-time walkthrough wizard (persona, OpenRouter"
  say "or Ollama, memory folders, the leash, vault password)…"
  if "$DEST_BIN/$BIN_NAME" setup; then
    ok "setup complete"
  else
    warn "wizard did not finish — run '$BIN_NAME setup' when ready"
  fi
else
  warn "not a terminal — run the wizard yourself: $BIN_NAME setup"
fi

echo
say "Done. Your agent lives encrypted at $DATA_DIR"
say "Start it any time with: $BIN_NAME"
