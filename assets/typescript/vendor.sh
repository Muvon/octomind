#!/usr/bin/env bash
# Rebuild the TypeScript compiler bundle octomind embeds for the `code` tool.
#
# The `code` tool type-checks every script before running it. The checker is
# the TypeScript compiler itself, run inside the JavaScript engine compiled
# into octomind, so no Node.js is needed on the user's machine. This script
# fetches one pinned release from the npm registry, verifies it against the
# registry's published checksum, and writes:
#
#   assets/typescript/typescript.js.zst  the compiler, unmodified, zstd-compressed
#   assets/typescript/lib.json.zst       the standard library typings (no DOM),
#                                        as a JSON object of file name -> text
#   assets/typescript/LICENSE.txt        the compiler's Apache-2.0 license
#
# TypeScript 6.0 is the last release written in JavaScript; 7.0 is a native
# program and cannot run inside the embedded engine.
#
# Usage: assets/typescript/vendor.sh
# Needs: curl, tar, zstd, python3, shasum

set -euo pipefail

VERSION="6.0.3"
OUT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

TARBALL="$WORK_DIR/typescript.tgz"
curl -fsSL "https://registry.npmjs.org/typescript/-/typescript-$VERSION.tgz" -o "$TARBALL"

# The registry publishes a sha512 of the tarball; refuse anything else.
EXPECTED="$(curl -fsSL "https://registry.npmjs.org/typescript/$VERSION" \
	| python3 -I -c 'import json, sys; print(json.load(sys.stdin)["dist"]["integrity"])')"
ACTUAL="sha512-$(shasum -a 512 "$TARBALL" | cut -d' ' -f1 | xxd -r -p | base64 | tr -d '\n')"
if [ "$EXPECTED" != "$ACTUAL" ]; then
	echo "typescript-$VERSION.tgz does not match the registry checksum" >&2
	echo "  expected $EXPECTED" >&2
	echo "  actual   $ACTUAL" >&2
	exit 1
fi

mkdir "$WORK_DIR/unpacked"
tar -xzf "$TARBALL" -C "$WORK_DIR/unpacked"
LIB_DIR="$WORK_DIR/unpacked/package/lib"

mkdir -p "$OUT_DIR"
zstd -19 -q -f "$LIB_DIR/typescript.js" -o "$OUT_DIR/typescript.js.zst"

# Scripts run in a bare engine: no browser, no worker, no Windows Script Host.
python3 -I - "$LIB_DIR" "$WORK_DIR/lib.json" <<'PY'
import json, os, sys
lib_dir, out = sys.argv[1], sys.argv[2]
skipped = ("dom", "webworker", "scripthost")
libs = {}
for name in sorted(os.listdir(lib_dir)):
    if name.startswith("lib.") and name.endswith(".d.ts") and not any(part in name for part in skipped):
        with open(os.path.join(lib_dir, name), encoding="utf-8") as handle:
            libs[name] = handle.read()
with open(out, "w", encoding="utf-8") as handle:
    json.dump(libs, handle, separators=(",", ":"))
PY
zstd -19 -q -f "$WORK_DIR/lib.json" -o "$OUT_DIR/lib.json.zst"

cp "$WORK_DIR/unpacked/package/LICENSE.txt" "$OUT_DIR/LICENSE.txt"

echo "typescript $VERSION vendored into $OUT_DIR:"
wc -c "$OUT_DIR"/typescript.js.zst "$OUT_DIR"/lib.json.zst
