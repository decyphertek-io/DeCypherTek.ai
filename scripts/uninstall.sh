#!/usr/bin/env bash
# DeCypherTek.ai — one-command uninstall.
#
# Removes exactly what scripts/install.sh created — and nothing else.
#
#   Linux (incl. the Podroid VM):
#     - the ~/.decyphertek.ai/bin folder (the binary — on Podroid it
#       also holds the docker->podman shim, removed with it).
#     - its marked PATH block in ~/.bashrc / ~/.zshrc (~/.profile too
#       on Alpine/Podroid, whose ash reads .profile).
#     - a legacy binary an older installer left in ~/.local/bin.
#     - podman and every other package the installer installed stay —
#       they are shared software.
#
# The encrypted vault in ~/.decyphertek.ai holds your agent — memory,
# config, keys — and is KEPT by default. Pass --purge to wipe it too.
set -euo pipefail

BIN_NAME="decyphertek.ai"
DATA_DIR="${HOME}/.decyphertek.ai"

say()  { printf '\033[36m>>\033[0m %s\n' "$*"; }
warn() { printf '\033[33m !\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m ✓\033[0m %s\n' "$*"; }
die()  { printf '\033[31m x\033[0m %s\n' "$*" >&2; exit 1; }

PURGE=0
case "${1:-}" in
  "") ;;
  --purge) PURGE=1 ;;
  -h|--help)
    sed -n '2,/^set -/p' "$0" | sed 's/^# \{0,1\}//; /^set -/d'
    exit 0
    ;;
  *) die "unknown option '$1' — only --purge is supported" ;;
esac

say "DeCypherTek.ai uninstaller"

# The agent's one folder minus the vault: the binary in
# ~/.decyphertek.ai/bin, its marked PATH block (also written to
# ~/.profile on Alpine/Podroid, whose ash reads .profile), and a legacy
# binary an older installer left in ~/.local/bin. Package installs
# (podman, …) stay — they are shared software.
if [[ -e "$HOME/.local/bin/$BIN_NAME" ]]; then
  rm -f "$HOME/.local/bin/$BIN_NAME"
  ok "removed legacy binary: $HOME/.local/bin/$BIN_NAME"
else
  ok "no legacy ~/.local/bin/$BIN_NAME (already gone)"
fi
if [[ -e "$DATA_DIR/bin" ]]; then
  rm -rf "$DATA_DIR/bin"
  ok "removed $DATA_DIR/bin"
else
  ok "no agent bin folder found at $DATA_DIR/bin (already gone)"
fi
for rc in "$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile"; do
  [[ -f "$rc" ]] || continue
  if grep -qF '.decyphertek.ai/bin' "$rc"; then
    grep -v -F \
      -e '# DeCypherTek.ai — agent bin on PATH (uninstall.sh removes this block)' \
      -e 'export PATH="$HOME/.decyphertek.ai/bin:$PATH"' \
      "$rc" > "$rc.tmp" || true
    cat "$rc.tmp" > "$rc"
    rm -f "$rc.tmp"
    ok "PATH block stripped from $rc"
  fi
done

# The encrypted vault: your agent's memory — kept unless --purge.
if [[ -e "$DATA_DIR" ]]; then
  if [[ "$PURGE" == 1 ]]; then
    rm -rf "$DATA_DIR"
    ok "purged $DATA_DIR — agent memory, config and keys are gone"
  else
    warn "kept $DATA_DIR — the encrypted vault holding your agent."
    warn "wipe it too by re-running with: --purge"
  fi
else
  ok "no agent vault found at $DATA_DIR"
fi

ok "DeCypherTek.ai uninstalled."
