#!/usr/bin/env bash
# Build the DeCypherTek.ai Android APK (Flutter + Rust).
#
# Separate from the terminal AI's pipeline on purpose: the repo root
# stays a plain Rust crate for Linux/macOS/Termux; this script only
# touches android/. The CI workflow (.github/workflows/Android-Build.yml)
# calls this script.
#
# Steps:
#   1. cargo-ndk builds android/core -> libdecyphertek_core.so (arm64-v8a)
#   2. the QEMU engine (libqemu.so) + UEFI firmware are placed next to it
#      in jniLibs/arm64-v8a (from --engine-dir, or a --engine-release tag)
#   3. flutter build apk --release
#
# Usage:
#   android/scripts/build-android.sh [--engine-dir dist] [--engine-release v0.1.0]
#        [--debug] [--split-per-abi]
#
# Requires: Rust + cargo-ndk + aarch64-linux-android rust target, the
# Android NDK (ANDROID_NDK_HOME), and Flutter.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
APP="$ROOT/android/app"
ENGINE_DIR=""
ENGINE_RELEASE=""
MODE="release"
EXTRA_FLUTTER_FLAGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --engine-dir) ENGINE_DIR="$2"; shift 2 ;;
    --engine-release) ENGINE_RELEASE="$2"; shift 2 ;;
    --debug) MODE="debug"; shift ;;
    --split-per-abi) EXTRA_FLUTTER_FLAGS+=(--split-per-abi); shift ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 1 ;;
  esac
done

say()  { printf '\033[36m>>\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m ✓\033[0m %s\n' "$*"; }
die()  { printf '\033[31m x\033[0m %s\n' "$*" >&2; exit 1; }

command -v cargo >/dev/null 2>&1 || die "Rust not installed (rustup.rs)"
command -v flutter >/dev/null 2>&1 || die "Flutter not installed (docs.flutter.dev)"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
[[ -n "$NDK" && -d "$NDK" ]] || die "ANDROID_NDK_HOME not set — install the Android NDK"
command -v cargo-ndk >/dev/null 2>&1 || die "cargo-ndk missing (cargo install cargo-ndk)"
rustup target list --installed 2>/dev/null | grep -q aarch64-linux-android \
  || die "rust target missing: rustup target add aarch64-linux-android"

# ---- 1. Rust core -> libdecyphertek_core.so ----
say "building the Rust core (aarch64)…"
JNILIBS="$APP/android/app/src/main/jniLibs/arm64-v8a"
mkdir -p "$JNILIBS"
cargo ndk -t arm64-v8a -o "$APP/android/app/src/main/jniLibs" build \
    --manifest-path "$ROOT/android/core/Cargo.toml" --release
ok "libdecyphertek_core.so built"

# ---- 2. QEMU engine + firmware into jniLibs ----

if [[ -n "$ENGINE_RELEASE" ]]; then
  say "fetching QEMU engine from release $ENGINE_RELEASE…"
  base="https://github.com/decyphertek-io/DeCypherTek.ai/releases/download/$ENGINE_RELEASE"
  curl -fL --retry 3 -o "$JNILIBS/libqemu.so" "$base/libqemu-arm64-v8a.so"
  curl -fL --retry 3 -o "$JNILIBS/libedk2-arm64.fd" "$base/libedk2-arm64.fd"
  curl -fL --retry 3 -o "$JNILIBS/libedk2-vars.fd" "$base/libedk2-vars.fd"
elif [[ -n "$ENGINE_DIR" ]]; then
  [[ -f "$ENGINE_DIR/libqemu-arm64-v8a.so" ]] || die "no libqemu-arm64-v8a.so in $ENGINE_DIR (run build-qemu-android.sh)"
  cp "$ENGINE_DIR/libqemu-arm64-v8a.so" "$JNILIBS/libqemu.so"
  for f in libedk2-arm64.fd libedk2-vars.fd; do
    [[ -f "$ENGINE_DIR/$f" ]] && cp "$ENGINE_DIR/$f" "$JNILIBS/$f" || true
  done
else
  die "no engine source — pass --engine-dir (build-qemu-android.sh output) or --engine-release <tag>"
fi
chmod +x "$JNILIBS/libqemu.so"
ok "engine staged in jniLibs/arm64-v8a"

# ---- 3. Flutter APK ----
say "building the APK ($MODE)…"
cd "$APP"
flutter pub get
if [[ "$MODE" == "release" ]]; then
  flutter build apk --release "${EXTRA_FLUTTER_FLAGS[@]}"
else
  flutter build apk --debug "${EXTRA_FLUTTER_FLAGS[@]}"
fi

APK="$APP/build/app/outputs/flutter-apk/app-$MODE.apk"
[[ -f "$APK" ]] || die "APK not produced"
mkdir -p "$ROOT/dist"
cp "$APK" "$ROOT/dist/"
(cd "$ROOT/dist" && sha256sum "$(basename "$APK")" > "$(basename "$APK").sha256")
ok "APK: $ROOT/dist/$(basename "$APK") ($(du -h "$APK" | cut -f1))"
