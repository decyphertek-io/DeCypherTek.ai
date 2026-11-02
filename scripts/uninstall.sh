#!/usr/bin/env bash
# DeCypherTek.ai — one-command uninstall.
#
# Removes everything the agent leaves in your home, in one step:
# the binary, the PATH lines it wired, and the whole
# ~/.decyphertek.ai folder — the encrypted vault with the agent's
# memory, config and keys included. There is no second step and no
# flag to remember: uninstall means gone.
#
# Package installs the installer added best-effort (podman, …) stay —
# they are shared system software, not the agent's.
#
#   Linux (incl. the Podroid VM).
set -euo pipefail

BIN_NAME="decyphertek.ai"
DATA_DIR="${HOME}/.decyphertek.ai"

say()  { printf '\033[36m>>\033[0m %s\n' "$*"; }
warn() { printf '\033[33m !\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m ✓\033[0m %s\n' "$*"; }
die()  { printf '\033[31m x\033[0m %s\n' "$*" >&2; exit 1; }

case "${1:-}" in
  "") ;;
  --purge)
    say "--purge is the default now — this uninstaller removes everything."
    ;;
  -h|--help)
    sed -n '2,/^set -/p' "$0" | sed 's/^# \{0,1\}//; /^set -/d'
    exit 0
    ;;
  *) die "unknown option '$1' — this uninstaller takes no options" ;;
esac

say "DeCypherTek.ai uninstaller — removes the agent AND its encrypted memory"

# A legacy binary an older installer left in ~/.local/bin.
if [[ -e "$HOME/.local/bin/$BIN_NAME" ]]; then
  rm -f "$HOME/.local/bin/$BIN_NAME"
  ok "removed legacy binary: $HOME/.local/bin/$BIN_NAME"
else
  ok "no legacy ~/.local/bin/$BIN_NAME (already gone)"
fi

# Its marked PATH block (also written to ~/.profile on Alpine/Podroid,
# whose ash reads .profile).
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

# The one folder: program files and the encrypted memory vault.
if [[ -e "$DATA_DIR" ]]; then
  rm -rf "$DATA_DIR"
  ok "removed $DATA_DIR — the agent, its memory, config and keys are gone"
else
  ok "nothing found at $DATA_DIR (already gone)"
fi

ok "DeCypherTek.ai uninstalled."
