#!/usr/bin/env bash
# Build the QEMU aarch64 system emulator for Android — the VM engine of
# the DeCypherTek.ai Android app.
#
# Output (in --out):
#   libqemu-arm64-v8a.so   -> bundled into the APK as jniLibs/arm64-v8a/libqemu.so
#                             (Android only extracts+execs binaries from
#                             nativeLibraryDir, hence the lib*.so name)
#   libedk2-arm64.fd       -> UEFI firmware (Armvirt, built from edk2)
#   libedk2-vars.fd        -> UEFI vars template
#   engine-meta.txt        -> qemu_version + engine_revision (sha256 of this
#                             script). The Android-Build workflow compares it
#                             with the vm-engine release to decide whether the
#                             published engine can be reused or must be rebuilt.
#
# Why a script and not a workflow step: the engine build runs only when the
# vm-engine release is missing or outdated (see Android-Build.yml's vm-engine
# job); the artifacts are attached to a GitHub Release and the APK build
# downloads them from there (see android/scripts/build-android.sh).
# Cross-compiling QEMU to Android needs libffi/pixman/glib built against
# the NDK first. libffi ships autotools only — no meson upstream — so it is
# the one configure/make exception in an otherwise all-meson build.
# libslirp and dtc resolve as QEMU's own git subprojects, cloned from
# gitlab.org via the wrap files QEMU ships (git smart-HTTP still works
# there; only the web-front pages are bot-gated).
#
# Usage:
#   android/scripts/build-qemu-android.sh --ndk /path/to/ndk [--api 28]
#        [--qemu-version 9.2.0] [--out dist] [--jobs N]
#        [--edk2 build|skip] [--edk2-tag edk2-stable202411]
#
# Requires: meson >= 1.5 (QEMU 9.2's floor; ubuntu 24.04's apt meson 1.3.2
# is too old — pipx install 'meson==1.6.1'), ninja, python3, pkg-config,
# curl, tar, git, make,
# and for --edk2 build: g++ + uuid-dev (BaseTools) and the
# gcc-aarch64-linux-gnu + iasl cross bits (apt-get install build-essential
# uuid-dev gcc-aarch64-linux-gnu acpica-tools).
set -euo pipefail

SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
# API 28, not the app's minSdk 24: bionic only ships iconv from API 28 and
# glib hard-requires it — the engine can only run on 9.0+ phones.
API=28
QEMU_VERSION="9.2.0"
EDK2_TAG="edk2-stable202411"
OUT="$(pwd)/dist"
JOBS="$(nproc 2>/dev/null || echo 4)"
EDK2="build"
WORK="$(pwd)/qemu-android-build"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --ndk) NDK="$2"; shift 2 ;;
    --api) API="$2"; shift 2 ;;
    --qemu-version) QEMU_VERSION="$2"; shift 2 ;;
    --edk2-tag) EDK2_TAG="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --jobs) JOBS="$2"; shift 2 ;;
    --edk2) EDK2="$2"; shift 2 ;;
    -h|--help) sed -n '2,35p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 1 ;;
  esac
done

say()  { printf '\033[36m>>\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m ✓\033[0m %s\n' "$*"; }
die()  { printf '\033[31m x\033[0m %s\n' "$*" >&2; exit 1; }

for tool in meson ninja python3 pkg-config curl tar git make; do
  command -v "$tool" >/dev/null 2>&1 || die "missing tool: $tool (apt-get install meson ninja-build python3 pkg-config curl tar git make)"
done
# QEMU >= 9.2 needs meson >= 1.5.0; Ubuntu 24.04's apt meson is 1.3.2
# and dies at setup. CI installs a current one via pipx.
meson_ver="$(meson --version)"
if [[ "$(printf '%s\n' "$meson_ver" '1.5.0' | sort -V | tail -1)" != "$meson_ver" ]]; then
  die "meson >= 1.5.0 required (QEMU $QEMU_VERSION), found $meson_ver — pipx install 'meson==1.6.1' or pip install meson"
fi

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

# ---- toolchain ----
# The NDK's API-suffixed clang wrappers (--target + built-in sysroot) are
# still shipped as of r29; if a future NDK drops them, fall back to the
# plain clang with explicit --target/--sysroot flags everywhere.
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
[[ -d "$TOOLCHAIN" ]] || TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin"
[[ -d "$TOOLCHAIN" ]] || die "unsupported NDK layout: $NDK"
SYSROOT="$(dirname "$TOOLCHAIN")/sysroot"

if [[ -x "$TOOLCHAIN/aarch64-linux-android${API}-clang" ]]; then
  CC="$TOOLCHAIN/aarch64-linux-android${API}-clang"
  CFLAGS="-fPIC"
else
  CC="$TOOLCHAIN/clang"
  CFLAGS="--target=aarch64-linux-android${API} --sysroot=$SYSROOT -fPIC"
  [[ -x "$CC" ]] || die "no aarch64 clang in $TOOLCHAIN"
fi

# Native file — pins meson's BUILD-machine compiler to the host `cc`.
# Without it meson reuses $CC for build-machine targets too: glib's
# internal tools (glib-compile-schemas & friends, built native:true)
# would then be compiled as aarch64 and linked against the host's
# x86_64 libz — "ld.lld: error: libz.so is incompatible with aarch64".
NATIVE="$WORK/host-native.txt"
cat > "$NATIVE" <<EOF
[binaries]
c   = 'cc'
cpp = 'c++'
EOF

CROSS="$WORK/android-cross.txt"
if [[ "$CFLAGS" == "-fPIC" ]]; then
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
else
  cat > "$CROSS" <<EOF
[binaries]
c     = ['${CC}', '--target=aarch64-linux-android${API}', '--sysroot=${SYSROOT}']
cpp   = ['${CC}++', '--target=aarch64-linux-android${API}', '--sysroot=${SYSROOT}']
ar    = '${TOOLCHAIN}/llvm-ar'
strip = '${TOOLCHAIN}/llvm-strip'
pkgconfig = 'pkg-config'

[host_machine]
system = 'android'
cpu_family = 'aarch64'
cpu = 'aarch64'
endian = 'little'
EOF
fi
ok "cross file: $CROSS"

fetch() { # fetch <url> <dest-tar> (curl + tar into cwd)
  local url="$1" dest="$2"
  if [[ ! -f "$dest" ]]; then
    say "downloading $(basename "$url")…"
    curl -fL --retry 3 "$url" -o "$dest"
  fi
  tar -xf "$dest"
}

# pkg-config must see ONLY our prefix — otherwise it walks the build
# machine's defaults and hands out the x86_64 zlib.pc (etc.) to the
# aarch64 lookups: "ld.lld: error: libz.so is incompatible with
# aarch64". Do not set PKG_CONFIG_SYSROOT_DIR either: prefix paths here
# are absolute, and a sysroot var would double them
# (-I$PREFIX/$PREFIX/include) — glib would never find libffi.
export CC CFLAGS
export AR="${TOOLCHAIN}/llvm-ar"
export RANLIB="${TOOLCHAIN}/llvm-ranlib"
export STRIP="${TOOLCHAIN}/llvm-strip"
export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig"
export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig"

# The NDK sysroot ships zlib (headers + static lib) but no pkg-config
# file, and glib/qemu hard-require one — synthesize it, pointing at the
# sysroot so the aarch64 link picks the real android zlib.
mkdir -p "$PREFIX/lib/pkgconfig"
cat > "$PREFIX/lib/pkgconfig/zlib.pc" <<EOF
Name: zlib
Description: zlib from the Android NDK sysroot
Version: 1.2.13
Libs: -L${SYSROOT}/usr/lib/aarch64-linux-android -lz
Cflags: -I${SYSROOT}/usr/include
EOF

cd "$WORK"

# ---- 1. libffi (glib dependency; upstream ships autotools only, no meson) ----
if [[ ! -d libffi-3.4.6 ]]; then
  fetch "https://github.com/libffi/libffi/releases/download/v3.4.6/libffi-3.4.6.tar.gz" libffi.tar.gz
fi
say "building libffi (configure/make — no meson upstream)…"
(
  cd libffi-3.4.6
  ./configure --host=aarch64-linux-android --prefix="$PREFIX" \
    --disable-shared --enable-static --disable-docs
  make -j"$JOBS"
  make install
)
ok "libffi built"

# ---- 2. pixman (QEMU softmmu dependency) ----
if [[ ! -d pixman-0.44.2 ]]; then
  fetch "https://cairographics.org/releases/pixman-0.44.2.tar.gz" pixman.tar.gz
fi
say "building pixman…"
meson setup pixman-build pixman-0.44.2 \
    --cross-file "$CROSS" --native-file "$NATIVE" --prefix "$PREFIX" \
    -Ddefault_library=static -Dtests=disabled -Ddemos=disabled >/dev/null
ninja -C pixman-build -j"$JOBS" >/dev/null
ninja -C pixman-build install >/dev/null
ok "pixman built"

# ---- 3. glib (QEMU's hard dependency) ----
if [[ ! -d glib-2.82.2 ]]; then
  fetch "https://download.gnome.org/sources/glib/2.82/glib-2.82.2.tar.xz" glib.tar.xz
fi
say "building glib (this takes a while)…"
# sysprof=disabled: keeps meson from cloning its sysprof wrap off
# gitlab.gnome.org — the engine never profiles, skip the extra network hop.
meson setup glib-build glib-2.82.2 \
    --cross-file "$CROSS" --native-file "$NATIVE" --prefix "$PREFIX" \
    -Ddefault_library=static \
    -Dtests=false -Dintrospection=disabled -Ddocumentation=false \
    -Dman=false -Dlibmount=disabled -Dlibelf=disabled \
    -Dsysprof=disabled >/dev/null
ninja -C glib-build -j"$JOBS" >/dev/null
ninja -C glib-build install >/dev/null
ok "glib built"

# ---- 4. QEMU aarch64 system emulator ----
if [[ ! -d "qemu-$QEMU_VERSION" ]]; then
  fetch "https://download.qemu.org/qemu-$QEMU_VERSION.tar.xz" qemu.tar.xz
fi
say "building QEMU $QEMU_VERSION (aarch64-softmmu)…"
# Bionic is "linux" enough that QEMU keeps POSIX-only backends QEMU-on-
# android cannot compile: bionic has no shm_open at any API (backend
# built for every non-Windows host), memfd_create only from API 30, and
# iommufd is host-side kernel API. None of those backends serve the
# VM's plain-RAM memory — drop them from the pinned source.
sed -i "/hostmem-shm\.c\|hostmem-memfd\.c\|host_iommu_device\.c/d" \
    "qemu-$QEMU_VERSION/backends/meson.build"
# QEMU drives meson itself — a bare `meson setup` dies on its missing
# config-host.mak, and its ./configure takes no --cross-file: it switches
# into cross mode via --cross-prefix and synthesizes config-meson.cross
# from env CC/AR/RANLIB/… (plus an implicit config-meson.native fixing
# the build machine at the host `cc`). Point every remaining binutil at
# the NDK's llvm tools, since ${cross_prefix}<tool> names don't exist.
mkdir -p qemu-build
(
  cd qemu-build
  # PKG_CONFIG must be exported or configure falls back to
  # ${cross_prefix}pkg-config — a binary that doesn't exist — and meson
  # then can't resolve glib/pixman for the host machine at all.
  export NM="$TOOLCHAIN/llvm-nm" READELF="$TOOLCHAIN/llvm-readelf" \
         OBJCOPY="$TOOLCHAIN/llvm-objcopy" DLLTOOL="$TOOLCHAIN/llvm-dlltool" \
         LD="$TOOLCHAIN/ld.lld" AS="$CC" CXX="${CC}++" PKG_CONFIG="pkg-config"
  ../"qemu-$QEMU_VERSION"/configure \
    --cross-prefix="aarch64-linux-android${API}-" \
    --cpu=aarch64 --target-list=aarch64-softmmu \
    -Ddefault_library=static \
    -Ddocs=disabled -Dtools=disabled -Dguest_agent=disabled \
    -Dsdl=disabled -Dgtk=disabled -Dopengl=disabled -Drutabaga_gfx=disabled \
    -Dvnc=disabled -Dspice=disabled -Dpa=disabled -Dpipewire=disabled \
    -Djack=disabled -Doss=disabled -Dsndio=disabled \
    -Dbrlapi=disabled -Dcurses=disabled \
    -Dcapstone=disabled -Dslirp=enabled -Dlibudev=disabled \
    -Dfuse=disabled -Dfuse_lseek=disabled -Dblkio=disabled \
    -Dcurl=disabled -Dglusterfs=disabled -Drbd=disabled \
    -Dvfio_user_server=disabled -Ddbus_display=disabled \
    -Dvhost_user=disabled -Dvhost_vdpa=disabled -Dvhost_kernel=disabled \
    -Dvirtfs=disabled
# Build just the engine binary: a bare `ninja` would also build the
# qtest suite and tools, whose glibc-leaning sources clash with bionic
# (e.g. 9p-marshal.h's st_atime_nsec vs the NDK's sys/stat.h macros).
)
# slirp stays ON: it is QEMU's user-mode networking — the piece that holds
# the loopback-only SSH forward. vnc stays OFF: nothing may ever bind 5900.
# The vhost family is off: android is "linux" enough for qemu to auto-pull
# libvhost-user, whose vendored headers then collide with the NDK's.
# Flag names are QEMU 9.2's (older lists used qemu_ga/udev/dbus/keyring/
# pulseaudio/virglrenderer, which meson 1.5+ rejects as unknown).
# libslirp + dtc (for aarch64's fdt) resolve as QEMU's own meson
# subprojects via the wrap files it ships — plain git clones from
# gitlab.org, no bot-gated web pages involved.
ninja -C qemu-build -j"$JOBS" qemu-system-aarch64
ok "QEMU built"

BIN="$WORK/qemu-build/qemu-system-aarch64"
[[ -f "$BIN" ]] || die "qemu-system-aarch64 not produced"
cp "$BIN" "$OUT/libqemu-arm64-v8a.so"
"$TOOLCHAIN/llvm-strip" "$OUT/libqemu-arm64-v8a.so" || true
ok "engine: $OUT/libqemu-arm64-v8a.so ($(du -h "$OUT/libqemu-arm64-v8a.so" | cut -f1))"

# ---- 5. UEFI firmware (ArmVirt, built from the pinned upstream edk2 tag;
# the old retrage/edk2-nightly prebuilts are gone) ----
case "$EDK2" in
  skip) ;;
  build)
    say "building EDK2 ArmVirtQemu firmware from $EDK2_TAG…"
    for dep in aarch64-linux-gnu-gcc iasl g++; do
      command -v "$dep" >/dev/null 2>&1 \
        || die "EDK2 build needs '$dep' (apt-get install build-essential uuid-dev gcc-aarch64-linux-gnu acpica-tools)"
    done
    [[ -d "$WORK/edk2" ]] || git clone --depth 1 --branch "$EDK2_TAG" https://github.com/tianocore/edk2.git "$WORK/edk2"
    git -C "$WORK/edk2" submodule update --init --depth 1
    (
      cd "$WORK/edk2"
      export WORKSPACE="$WORK/edk2-work"
      export PACKAGES_PATH="$WORK/edk2"
      export GCC5_AARCH64_PREFIX=aarch64-linux-gnu-
      make -C BaseTools -j"$JOBS"
      source edksetup.sh
      build -a AARCH64 -t GCC5 -p ArmVirtPkg/ArmVirtQemu.dsc -b RELEASE -n "$JOBS"
      cp Build/ArmVirtQemu-AARCH64/RELEASE_GCC5/FV/QEMU_EFI.fd "$OUT/libedk2-arm64.fd"
      cp Build/ArmVirtQemu-AARCH64/RELEASE_GCC5/FV/QEMU_VARS.fd "$OUT/libedk2-vars.fd"
    )
    ok "firmware built from edk2 $EDK2_TAG"
    ;;
  *) die "unknown --edk2 mode: $EDK2 (build or skip — the old download mode is gone, retrage/edk2-nightly stopped publishing)" ;;
esac

# ---- 6. engine identity — what Android-Build compares against the
# vm-engine release to decide reuse-vs-rebuild ----
{
  echo "qemu_version=$QEMU_VERSION"
  echo "engine_revision=$(sha256sum "$SELF" | cut -d' ' -f1)"
  echo "edk2_tag=$EDK2_TAG"
} > "$OUT/engine-meta.txt"

echo
ok "done. artifacts in $OUT:"
ls -lh "$OUT"
