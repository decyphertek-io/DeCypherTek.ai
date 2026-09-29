#!/usr/bin/env bash
# DeCypherTek.ai — one-command uninstall.
#
# Removes exactly what scripts/install.sh created — and nothing else.
#
#   Termux:
#     - the Debian proot instance the installer custom-named
#       'decyphertek' — removable by name, so a Debian (or any other)
#       proot you installed yourself is never touched.
#     - legacy instances earlier installers left under the plain
#       'debian' / 'archlinux' aliases — removed only when they carry
#       the DeCypherTek fingerprint (our launcher/binary inside the
#       rootfs); your own keep that fingerprint absent and survive.
#     - the decyphertek.ai launcher in $PREFIX/bin and the sourced
#       alias in ~/.bashrc / ~/.zshrc.
#
#   Desktop:
#     - the ~/.local/bin/decyphertek.ai binary. The PATH export line,
#       Docker and every other package it installed stay.
#
# The encrypted vault in ~/.decyphertek.ai holds your agent — memory,
# config, keys — and is KEPT by default. Pass --purge to wipe it too.
set -euo pipefail

BIN_NAME="decyphertek.ai"
DATA_DIR="${HOME}/.decyphertek.ai"
# The installer's Debian instance, under its dedicated alias — removing
# it can never collide with a user-installed Debian proot.
PROOT_DISTRO="decyphertek"

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

TERMUX=0
if [[ -n "${TERMUX_VERSION:-}" || "${PREFIX:-}" == *com.termux* ]]; then TERMUX=1; fi

need() { command -v "$1" >/dev/null 2>&1; }

# proot-distro layouts: v5 keeps containers/<alias>/rootfs, v4 used
# installed-rootfs/<alias>.
pd_rootfs_of() {  # $1 = alias -> prints existing rootfs path, else rc 1
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

# Ours or the user's? Every release of the installer wrote the
# dct-launch helper / the decyphertek.ai binary into the rootfs's
# /usr/local/bin — that fingerprint is what authorizes a removal.
ours_by_fingerprint() {  # $1 = rootfs path
  [[ -f "$1/usr/local/bin/dct-launch" ]] || [[ -f "$1/usr/local/bin/$BIN_NAME" ]]
}

say "DeCypherTek.ai uninstaller"

if [[ "$TERMUX" == 1 ]]; then
  # 1) the agent's own Debian proot instance — dedicated 'decyphertek'
  #    alias: removable by name, never a user's separate Debian.
  if root="$(pd_rootfs_of "$PROOT_DISTRO")"; then
    if ours_by_fingerprint "$root"; then
      proot-distro remove "$PROOT_DISTRO" --force >/dev/null 2>&1 \
        && ok "removed the '$PROOT_DISTRO' Debian proot instance" \
        || warn "proot-distro could not remove it — run: proot-distro remove $PROOT_DISTRO --force"
    else
      warn "a proot instance named '$PROOT_DISTRO' exists but carries no DeCypherTek"
      warn "fingerprint — it was not installed by DeCypherTek; leaving it alone."
    fi
  else
    ok "no '$PROOT_DISTRO' Debian proot instance (already gone)"
  fi

  # 2) legacy instances earlier installers left behind, under stock
  #    aliases. Fingerprinted → ours → remove; the user's own Debian or
  #    Arch proots (no fingerprint) survive.
  for alias in debian archlinux; do
    if root="$(pd_rootfs_of "$alias")"; then
      if ours_by_fingerprint "$root"; then
        proot-distro remove "$alias" --force >/dev/null 2>&1 \
          && ok "removed the older '$alias' instance left by a previous installer" \
          || warn "proot-distro could not remove '$alias' — run: proot-distro remove $alias --force"
      else
        ok "kept '$alias' — it is not DeCypherTek's"
      fi
    fi
  done

  # 3) the Termux-side launcher (and the legacy pre-alias one).
  for f in "$PREFIX/bin/$BIN_NAME" "$PREFIX/bin/decyphertek"; do
    if [[ -e "$f" ]]; then
      rm -f "$f"
      ok "removed $f"
    fi
  done

  # 4) the sourced alias lines from the shell rc files.
  for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
    [[ -f "$rc" ]] || continue
    if grep -q '^alias decyphertek\.ai=' "$rc"; then
      sed -i \
        -e '\|^# DeCypherTek.ai — from a Termux prompt straight into the proot Debian home$|d' \
        -e '\|^alias decyphertek\.ai=|d' "$rc"
      ok "alias stripped from $rc"
    fi
  done
  warn "already-open shells keep the old alias in memory — open a new Termux"
  warn "shell for it to be gone everywhere."
else
  # Desktop: the installed binary. PATH exports, package installs and
  # anything else the installer touched stay — they are shared software.
  f="$HOME/.local/bin/$BIN_NAME"
  if [[ -e "$f" ]]; then
    rm -f "$f"
    ok "removed $f"
  else
    ok "no desktop binary found (already gone)"
  fi
fi

# 5) the encrypted vault: your agent's memory — kept unless --purge.
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
