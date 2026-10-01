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
# Who calls this: .github/actions/build-qemu-engine (a composite action)
# which Android-Build invokes ONLY on an engine-cache miss — a new QEMU
# version, a forced rebuild, or a changed build script. Same version
# means same cached engine; nothing rebuilds. For manual/one-off builds:
#
#   android/scripts/build-qemu-android.sh --ndk /path/to/ndk [--qemu-version 9.2.0]
#        [--api 28] [--out engine] [--jobs N] [--edk2 download|build|skip]
#
# Cross-compiling QEMU to Android needs glib/pixman/libffi against the
# NDK first — all meson, one cross file, one prefix.
#
# Requires: meson >= 1.0, ninja, python3, pkg-config, curl, tar, git.
set -euo pipefail

NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
API=28
QEMU_VERSION="9.2.0"
OUT="$(pwd)/engine"
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
    -h|--help) sed -n '2,24p' "$0"; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 1 ;;
  esac
done

say()  { printf '\033[36m>>\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m ✓\033[0m %s\n' "$*"; }
die()  { printf '\033[31m x\033[0m %s\n' "$*" >&2; exit 1; }

for tool in meson ninja python3 pkg-config curl tar git; do
  command -v "$tool" >/dev/null 2>&1 || die "missing tool: $tool (apt-get install meson ninja-build python3 pkg-config curl tar git)"
done

[[ -n "$NDK" && -d "$NDK" ]] || die "no Android NDK found — pass --ndk /path/to/ndk (or set ANDROID_NDK_HOME)"
ok "NDK: $NDK"

mkdir -p "$OUT"
PREFIX="$WORK/prefix"
mkdir -p "$PREFIX" "$WORK"

# ---- meson cross file (shared by libffi, pixman, glib, qemu) ----
TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
[[ -d "$TOOLCHAIN" ]] || TOOLCHAIN="$NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin"
[[ -d "$TOOLCHAIN" ]] || die "unsupported NDK layout: $NDK"
CC="$TOOLCHAIN/aarch64-linux-android${API}-clang"
[[ -x "$CC" ]] || die "clang wrapper not found: $CC (wrong NDK or API level?)"

CROSS="$WORK/android-cross.txt"
cat > "$CROSS" <<EOF
[binaries]
c     = '${CC}'
cpp   = '${CC}++'
ar    = '${TOOLCHAIN}/llvm-ar'
strip = '${TOOLCHAIN}/llvm-strip'

[host_machine]
system = 'android'
cpu_family = 'aarch64'
cpu = 'aarch64'
endian = 'little'
EOF
ok "cross file: $CROSS"

# ---- cross pkg-config: prefix ONLY ----
# PKG_CONFIG_LIBDIR *replaces* the system search dirs — without it the
# runner's x86_64 libs get offered to an arm64 cross build (wrong-arch
# linking or mystery feature detection). With it, anything not in the
# prefix is simply "not found" and auto-features disable cleanly.
export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig"
export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig"
export PKG_CONFIG_SYSROOT_DIR="$PREFIX"

fetch() { # fetch <url> <tarball> (into cwd)
  local url="$1" dest="$2"
  if [[ ! -f "$dest" ]]; then
    say "downloading $(basename "$url")…"
    curl -fL --retry 3 "$url" -o "$dest"
  fi
  tar -xf "$dest"
}

# Quiet on success, TREMENDOUSLY useful on failure: ninja output goes to
# a log file; on failure the tail is printed and we die. (The original
# failure was invisible because meson's errors were piped to /dev/null.)
LOG="$WORK/last-build.log"
build_step() { # build_step <builddir> <label>
  local dir="$1" label="$2"
  if ! ninja -C "$dir" -j"$JOBS" > "$LOG" 2>&1; then
    tail -n 120 "$LOG" >&2
    die "ninja failed: $label (last 120 lines above; full log: $LOG)"
  fi
}

cd "$WORK"

# ---- 1. libffi (glib dependency) ----
if [[ ! -d libffi-* ]]; then
  fetch "https://github.com/libffi/libffi/releases/download/v3.4.6/libffi-3.4.6.tar.gz" libffi.tar.gz
fi
say "configuring + building libffi…"
meson setup libffi-build libffi-3.4.6 \
    --cross-file "$CROSS" --prefix "$PREFIX" \
    -Ddefault_library=static
build_step libffi-build "libffi"
ninja -C libffi-build install > /dev/null
ok "libffi built"

# ---- 2. pixman (QEMU softmmu dependency) ----
if [[ ! -d pixman-* ]]; then
  fetch "https://pixman.org/releases/pixman-0.44.2.tar.gz" pixman.tar.gz
fi
say "configuring + building pixman…"
meson setup pixman-build pixman-0.44.2 \
    --cross-file "$CROSS" --prefix "$PREFIX" \
    -Ddefault_library=static -Dtests=disabled -Ddemos=disabled -Dgnuplot=disabled
build_step pixman-build "pixman"
ninja -C pixman-build install > /dev/null
ok "pixman built"

# ---- 3. glib (QEMU's hard dependency) ----
if [[ ! -d glib-* ]]; then
  fetch "https://download.gnome.org/sources/glib/2.82/glib-2.82.2.tar.xz" glib.tar.xz
fi
say "configuring + building glib (this takes a while)…"
meson setup glib-build glib-2.82.2 \
    --cross-file "$CROSS" --prefix "$PREFIX" \
    -Ddefault_library=static \
    -Dtests=false -Dintrospection=disabled -Ddocumentation=false \
    -Dman=false -Dlibmount=disabled -Dlibelf=disabled -Dnls=disabled
build_step glib-build "glib"
ninja -C glib-build install > /dev/null
ok "glib built"

# ---- 4. QEMU aarch64 system emulator ----
if [[ ! -d "qemu-$QEMU_VERSION" ]]; then
  fetch "https://download.qemu.org/qemu-$QEMU_VERSION.tar.xz" qemu.tar.xz
fi
say "configuring QEMU $QEMU_VERSION (aarch64-softmmu)…"
# Option set is deliberately conservative: only flags that certainly
# exist in QEMU 9.2.x. Everything else is left auto — under the
# prefix-only pkg-config above the host libs are invisible, so missing
# features just disable themselves instead of linking wrong-arch junk.
# slirp stays ON: it is QEMU's user-mode networking — the piece that
# holds the loopback-only SSH forward. vnc stays OFF: nothing may ever
# bind 5900.
meson setup qemu-build "qemu-$QEMU_VERSION" \
    --cross-file "$CROSS" --prefix "$PREFIX" \
    -Ddefault_library=static \
    -Dtarget_list=aarch64-softmmu \
    -Ddocs=disabled -Dtools=disabled -Dguest_agent=disabled \
    -Dsdl=disabled -Dgtk=disabled -Dopengl=disabled \
    -Dvnc=disabled -Dspice=disabled -Dcurses=disabled \
    -Dcapstone=disabled -Dfuse=disabled -Dfuse_lseek=disabled \
    -Dblkio=disabled -Dkeyring=disabled \
    -Dslirp=enabled
build_step qemu-build "QEMU"
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
    # Firmware comes from the distro's own qemu-efi-aarch64 package:
    # firmware is ROM data — host arch is irrelevant — and the Debian/
    # Ubuntu archive is a stable source (the old nightly-bin repo is
    # gone). QEMU_EFI.fd + QEMU_VARS.fd are exactly ArmVirtQemu's pair.
    say "fetching UEFI firmware (qemu-efi-aarch64 from the archive)…"
    PKGDIR="$WORK/edk2-pkg"
    mkdir -p "$PKGDIR"
    if ( cd "$PKGDIR" && apt-get download qemu-efi-aarch64 >/dev/null 2>&1 ); then
      dpkg-deb -x "$PKGDIR"/qemu-efi-aarch64_*.deb "$PKGDIR/root"
      cp "$PKGDIR/root/usr/share/qemu-efi-aarch64/QEMU_EFI.fd" "$OUT/libedk2-arm64.fd"
      cp "$PKGDIR/root/usr/share/qemu-efi-aarch64/QEMU_VARS.fd" "$OUT/libedk2-vars.fd"
      ok "firmware extracted from the qemu-efi-aarch64 package"
    else
      die "could not fetch qemu-efi-aarch64 — run with apt indexes present, or use --edk2 build"
    fi
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

(cd "$OUT" && sha256sum * > checksums.txt)
echo
ok "done. artifacts in $OUT:"
ls -lh "$OUT"
