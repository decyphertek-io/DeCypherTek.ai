//! Build script — compiles the vendored SQLite-Vector extension
//! (github.com/sqliteai/sqlite-vector, Apache-2.0, see
//! vendor/sqlite-vector/LICENSE.md) straight into the binary.
//!
//! SQLITE_CORE means static embedding, not a loadable .so: the extension
//! resolves sqlite3_* symbols against the SQLite that rusqlite's
//! `bundled` feature already compiles in, which keeps the one-static-
//! musl-binary deployment intact. `Vectors::open` registers it on every
//! connection via sqlite3_auto_extension (src/vector.rs).
//!
//! The AVX2/AVX-512 translation units are compiled separately with their
//! ISA flags, mirroring the extension's own Makefile: the kernels are
//! guarded by __AVX2__/__AVX512F__ inside, and a runtime CPU check picks
//! the best set the machine supports (scalar/SSE2/NEON elsewhere).

use std::path::Path;

fn main() {
    let root = Path::new("vendor/sqlite-vector");
    let src = root.join("src");
    let libs = root.join("libs");

    let target = std::env::var("TARGET").unwrap_or_default();
    let is_x86 = target.starts_with("x86_64") || target.starts_with("i686");

    let mut base = cc::Build::new();
    base.include(&src)
        .include(&libs)
        .define("SQLITE_CORE", None)
        .opt_level(3)
        .warnings(false)
        .file(src.join("sqlite-vector.c"))
        .file(src.join("distance-cpu.c"))
        .file(src.join("distance-sse2.c"))
        .file(src.join("distance-neon.c"))
        .file(src.join("distance-rvv.c"))
        .compile("sqlite_vector_base");

    if is_x86 {
        let mut avx2 = cc::Build::new();
        avx2.include(&src)
            .include(&libs)
            .define("SQLITE_CORE", None)
            .opt_level(3)
            .warnings(false)
            .flag_if_supported("-mavx2")
            .flag_if_supported("-mfma")
            .file(src.join("distance-avx2.c"))
            .compile("sqlite_vector_avx2");

        let mut avx512 = cc::Build::new();
        avx512
            .include(&src)
            .include(&libs)
            .define("SQLITE_CORE", None)
            .opt_level(3)
            .warnings(false)
            .flag_if_supported("-mavx512f")
            .flag_if_supported("-mavx512bw")
            .flag_if_supported("-mavx512vl")
            .flag_if_supported("-mavx512dq")
            .file(src.join("distance-avx512.c"))
            .compile("sqlite_vector_avx512");
    }

    println!("cargo:rerun-if-changed=vendor/sqlite-vector/src");
    println!("cargo:rerun-if-changed=vendor/sqlite-vector/libs");
}
