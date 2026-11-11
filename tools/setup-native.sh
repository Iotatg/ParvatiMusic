#!/usr/bin/env bash
# Fetch the prebuilt libntgcalls static archive the FFI crate links against.
#
# The voice engine is C++ and its source build pulls WebRTC + BoringSSL, which
# takes hours; ntgcalls publishes ready-made archives instead. This script only
# needs the v2.2.x line — v3.x changed the C API (ntg_init/ntg_create_call) and
# will not link against `ntg/src/lib.rs`.
set -euo pipefail

VER="${NTGCALLS_VERSION:-v2.2.5}"
ARCH="${NTGCALLS_ARCH:-linux-x86_64}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/ntg/native"

mkdir -p "$DEST"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

url="https://github.com/pytgcalls/ntgcalls/releases/download/${VER}/ntgcalls.${ARCH}-static_libs.zip"
echo "→ $url"
curl -fsSL -o "$tmp/ntg.zip" "$url"
unzip -q -o "$tmp/ntg.zip" -d "$tmp/ext"

cp "$tmp/ext/lib/libntgcalls.a" "$DEST/libntgcalls_trimmed.a"

# build.rs links this one with +whole-archive; the upstream release already
# carries BoringSSL inside the main archive, so an empty stub is enough.
printf 'void parvati_boringssl_forced_placeholder(void){}\n' > "$tmp/empty.c"
cc -c "$tmp/empty.c" -o "$tmp/empty.o"
ar rcs "$DEST/libntgcalls_boringssl_forced.a" "$tmp/empty.o"

# libntgcalls' bundled c-ares calls the unversioned __res_nquery/__dn_expand,
# which glibc only exports as versioned symbols — the shim restores them.
cat > "$DEST/resolver_compat.c" <<'EOF'
#include <arpa/nameser.h>
#include <netinet/in.h>
#include <resolv.h>

int __res_nquery(res_state state, const char *dname, int class, int type,
                 unsigned char *answer, int anslen) {
    (void)state;
    return res_query(dname, class, type, answer, anslen);
}

int __dn_expand(const unsigned char *msg, const unsigned char *eom,
                const unsigned char *src, char *dst, int dstsiz) {
    return dn_expand(msg, eom, src, dst, dstsiz);
}
EOF

echo "✓ $DEST"
ls -lh "$DEST"
echo
echo "next: cargo build --release -p parvati_bot"
