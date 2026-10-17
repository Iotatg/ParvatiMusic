use std::path::Path;

fn main() {
    let dir = Path::new("native");
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dir = if dir.exists() { manifest.join(dir) } else { manifest.join("native") };

    // resolver_compat shims __res_nquery/__dn_expand for libntgcalls' bundled
    // c-ares against glibc >= 2.34 (mirrors src/ntgcalls/resolver_compat.c).
    cc::Build::new()
        .file(dir.join("resolver_compat.c"))
        .compile("resolver_compat");

    println!("cargo:rustc-link-search=native={}", dir.display());
    // Whole-archive the BoringSSL-forced subset first so ASN1_*/d2i_* resolve
    // against libntgcalls' bundled objects instead of system libcrypto
    // (mirrors CMakeLists.txt lines 102-117).
    println!(
        "cargo:rustc-link-lib=static:+whole-archive=ntgcalls_boringssl_forced"
    );
    println!("cargo:rustc-link-lib=static=ntgcalls_trimmed");
    println!("cargo:rustc-link-lib=m");
    println!("cargo:rustc-link-lib=z");
    println!("cargo:rustc-link-lib=resolv");
    println!("cargo:rustc-link-lib=dl");
    println!("cargo:rustc-link-lib=stdc++");
    println!("cargo:rustc-link-lib=pthread");

    println!("cargo:rerun-if-changed=native/libntgcalls_trimmed.a");
    println!("cargo:rerun-if-changed=native/libntgcalls_boringssl_forced.a");
    println!("cargo:rerun-if-changed=native/resolver_compat.c");
}
