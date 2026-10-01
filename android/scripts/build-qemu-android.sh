#!/usr/bin/env bash
# Build the QEMU aarch64 system emulator for Android — the VM engine of
# the DeCypherTek.ai Android app.
#
# Output (in --out):
#   libqemu-arm64-v8a.so   -> bundled into the APK as jniLibs/arm64-v8a/libqemu.so
#                             (Android only extracts+execs binaries from
#                             nativeLibraryDir, hence the lib*.so name)
#   libedk2-arm64.fd       -> UEFI firmware (optional; also downloadable)
#   libedk2-vars.fd        -> UEFI vars template (optional)
#
# Why a script and not a workflow step: this is run ONCE per engine
# release; the artifacts are attached to a GitHub Release and the
# Android-Build workflow downloads them (see android/scripts/build-android.sh).
# Cross-compiling QEMU to Android needs glib/pixman/libffi built against
# the NDK first — all meson, one cross file, three prefixes.
#
# Usage:
#   android/scripts/build-qemu-android.sh --ndk /path/to/ndk [--api 28]
#        [--qemu-version 9.2.0] [--out dist] [--jobs N] [--edk2 download|build|skip]
#
# Requires: meson >= 1.0, ninja, python3, pkg-config, curl, tar, git.
set -euo pipefail

NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
API=28
QEMU_VERSION="9.2.0"
OUT="$(pwd)/dist"
JOBS="$(nproc 2>/dev/null || echo 4)"
EDK2="download"
WORK="$(pwd)/qemu-android-build"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --ndk) NDK="$2"; shift 2 ;;
    --api) API="$2"; shift 2 ;;
    --qemu-version) QEMU_VERSION="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --jobs) JOBS="$2"; shift 2 ;;
    --edk2) EDK2="$2"; shift 2 ;;
    -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 1 ;;
  esac
done

say()  { printf '\033[36m>>\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m ✓\033[0m %s\n' "$*"; }
die()  { printf '\033[31m x\033[0m %s\n' "$*" >&2; exit 1; }

for tool in meson ninja python3 pkg-config curl tar git; do
  command -v "$tool" >/dev/null 2>&1 || die "missing tool: $tool (apt-get install meson ninja-build python3 pkg-config curl tar git)"
done

if [[ -z "$NDK" || ! -d "$NDK" ]]; then
  for candidate in "$HOME/Android/Sdk/ndk/"* /opt/android-ndk*; do
    if [[ -d "$candidate" ]]; then NDK="$candidate"; break; fi
  done
fi
[[ -n "$NDK" && -d "$NDK" ]] || die "no Android NDK found — pass --ndk /path/to/ndk"
ok "NDK: $NDK"

mkdir -p "$WORK" "$OUT"
PREFIX="$WORK/prefix"
mkdir -p "$PREFIX"

# ---- meson cross file (shared by libffi, pixman, glib, qemu) ----
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
[[ -d "$TOOLCHAIN" ]] || TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin"
[[ -d "$TOOLCHAIN" ]] || die "unsupported NDK layout: $NDK"
CC="$TOOLCHAIN/aarch64-linux-android${API}-clang"

CROSS="$WORK/android-cross.txt"
cat > "$CROSS" <<EOF
[binaries]
c     = '${CC}'
cpp   = '${CC}++'
ar    = '${TOOLCHAIN}/llvm-ar'
strip = '${TOOLCHAIN}/llvm-strip'
pkgconfig = 'pkg-config'

[host_machine]
system = 'android'
cpu_family = 'aarch64'
cpu = 'aarch64'
endian = 'little'
EOF
ok "cross file: $CROSS"

fetch() { # fetch <url> <dest-tar> (curl + tar into cwd)
  local url="$1" dest="$2"
  if [[ ! -f "$dest" ]]; then
    say "downloading $(basename "$url")…"
    curl -fL --retry 3 "$url" -o "$dest"
  fi
  tar -xf "$dest"
}

export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig"
export PKG_CONFIG_SYSROOT_DIR="$PREFIX"
export CC CFLAGS="--sysroot=$(dirname "$(dirname "$CC")")/sysroot -fPIC"

cd "$WORK"

# ---- 1. libffi (glib dependency) ----
if [[ ! -d libffi-* ]]; then
  fetch "https://github.com/libffi/libffi/releases/download/v3.4.6/libffi-3.4.6.tar.gz" libffi.tar.gz
fi
say "building libffi…"
meson setup libffi-build libffi-3.4.6 \
    --cross-file "$CROSS" --prefix "$PREFIX" \
    -Ddefault_library=static >/dev/null
ninja -C libffi-build -j"$JOBS" >/dev/null
ninja -C libffi-build install >/dev/null
ok "libffi built"

# ---- 2. pixman (QEMU softmmu dependency) ----
if [[ ! -d pixman-* ]]; then
  fetch "https://pixman.org/releases/pixman-0.44.2.tar.gz" pixman.tar.gz
fi
say "building pixman…"
meson setup pixman-build pixman-0.44.2 \
    --cross-file "$CROSS" --prefix "$PREFIX" \
    -Ddefault_library=static -Dtests=disabled -Ddemos=disabled >/dev/null
ninja -C pixman-build -j"$JOBS" >/dev/null
ninja -C pixman-build install >/dev/null
ok "pixman built"

# ---- 3. glib (QEMU's hard dependency) ----
if [[ ! -d glib-* ]]; then
  fetch "https://download.gnome.org/sources/glib/2.82/glib-2.82.2.tar.xz" glib.tar.xz
fi
say "building glib (this takes a while)…"
meson setup glib-build glib-2.82.2 \
    --cross-file "$CROSS" --prefix "$PREFIX" \
    -Ddefault_library=static \
    -Dtests=false -Dintrospection=disabled -Ddocumentation=false \
    -Dman=false -Dlibmount=disabled -Dlibelf=disabled >/dev/null
ninja -C glib-build -j"$JOBS" >/dev/null
ninja -C glib-build install >/dev/null
ok "glib built"

# ---- 4. QEMU aarch64 system emulator ----
if [[ ! -d "qemu-$QEMU_VERSION" ]]; then
  fetch "https://download.qemu.org/qemu-$QEMU_VERSION.tar.xz" qemu.tar.xz
fi
say "building QEMU $QEMU_VERSION (aarch64-softmmu)…"
meson setup qemu-build "qemu-$QEMU_VERSION" \
    --cross-file "$CROSS" --prefix "$PREFIX" \
    -Ddefault_library=static \
    -Ddocs=disabled -Dtools=disabled -Dguest_agent=disabled \
    -Dsdl=disabled -Dgtk=disabled -Dopengl=disabled -Dvirglrenderer=disabled \
    -Dvnc=disabled -Dspice=disabled -Dpa=disabled -Dpipewire=disabled \
    -Djack=disabled -Doss=disabled -Dsndio=disabled -Dpulseaudio=disabled \
    -Dbrlapi=disabled -Dcurses=disabled -Dgtk=disabled \
    -Dcapstone=disabled -Dslirp=enabled -Dudev=disabled \
    -Dfuse=disabled -Dfuse_lseek=disabled -Dblkio=disabled \
    -Dcurl=disabled -Dglusterfs=disabled -Drbd=disabled \
    -Dvfio_user_server=disabled -Ddbus=disabled -Dkeyring=disabled \
    -Dqemu_ga=disabled >/dev/null
# slirp stays ON: it is QEMU's user-mode networking — the piece that holds
# the loopback-only SSH forward. vnc stays OFF: nothing may ever bind 5900.
ninja -C qemu-build -j"$JOBS"
ok "QEMU built"

BIN="$WORK/qemu-build/qemu-system-aarch64"
[[ -f "$BIN" ]] || die "qemu-system-aarch64 not produced"
cp "$BIN" "$OUT/libqemu-arm64-v8a.so"
"$TOOLCHAIN/llvm-strip" "$OUT/libqemu-arm64-v8a.so" || true
ok "engine: $OUT/libqemu-arm64-v8a.so ($(du -h "$OUT/libqemu-arm64-v8a.so" | cut -f1))"

# ---- 5. UEFI firmware ----
case "$EDK2" in
  skip) ;;
  download)
    say "downloading prebuilt EDK2 ArmVirt firmware…"
    curl -fL --retry 3 -o "$OUT/libedk2-arm64.fd" \
      "https://github.com/retrage/edk2-nightly/releases/latest/download/edk2-arm64-QEMU_EFI.fd"
    curl -fL --retry 3 -o "$OUT/libedk2-vars.fd" \
      "https://github.com/retrage/edk2-nightly/releases/latest/download/edk2-arm64-vars.fd"
    ok "firmware downloaded"
    ;;
  build)
    say "building EDK2 ArmVirtQemu from source…"
    if ! command -v aarch64-linux-gnu-gcc >/dev/null 2>&1; then
      die "EDK2 source build needs gcc-aarch64-linux-gnu (apt-get install gcc-aarch64-linux-gnu)"
    fi
    git clone --depth 1 --branch edk2-stable202411 https://github.com/tianocore/edk2.git "$WORK/edk2" || true
    git -C "$WORK/edk2" submodule update --init
    (
      cd "$WORK/edk2"
      export WORKSPACE="$WORK/edk2-work"
      export PACKAGES_PATH="$WORK/edk2"
      make -C BaseTools -j"$JOBS"
      source edksetup.sh
      build -a AARCH64 -t GCC5 -p ArmVirtPkg/ArmVirtQemu.dsc -b RELEASE
      cp Build/ArmVirtQemu-AARCH64/RELEASE_GCC5/FV/QEMU_EFI.fd "$OUT/libedk2-arm64.fd"
      cp Build/ArmVirtQemu-AARCH64/RELEASE_GCC5/FV/QEMU_VARS.fd "$OUT/libedk2-vars.fd"
    )
    ok "firmware built from source"
    ;;
  *) die "unknown --edk2 mode: $EDK2" ;;
esac

echo
ok "done. artifacts in $OUT:"
ls -lh "$OUT"
