# parvati-ntg

Safe Rust FFI over the `libntgcalls` native voice engine.

## Native libraries (gitignored)

`build.rs` expects three static archives in `native/`:

- `libntgcalls_trimmed.a` — the engine archive
- `libntgcalls_boringssl_forced.a` — BoringSSL objects, linked
  `+whole-archive` so the engine's required symbols survive the linker
- `libresresolver_compat` is compiled from `native/resolver_compat.c`
  (`__res_nquery` / `__dn_expand` shim) by the `cc` crate

Build/copy the `libntgcalls*.a` archives from your own voice-engine
toolchain and place them here:

```
cp /path/to/libntgcalls*.a ntg/native/
```

`cargo:rustc-link-search` is pointed at `native/` automatically; system
link line: `m z resolv dl stdc++ pthread`.
