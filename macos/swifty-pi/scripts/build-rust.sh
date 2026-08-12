#!/usr/bin/env bash
# Builds pi-core-ffi, generates its Swift bindings, and packages everything
# into PiCoreFFI.xcframework + Generated/pi_core_ffi.swift for the SwiftyPi
# Xcode target. Runs as an Xcode "Run Script" build phase (see
# SwiftyPi.xcodeproj) or standalone from a terminal for debugging — it does
# nothing Xcode-specific, so failures can be reproduced and iterated on here.
#
# Host-architecture only for this spike (SW1): a universal arm64+x86_64
# build (two `cargo build --target`s + `lipo`, or multiple `-library` entries
# on `-create-xcframework`) is scoped to a later milestone once the FFI
# surface has stabilized — see docs/plans/SW1-ffi-spike-and-chat-window.md.
. ~/.cargo/env

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT="$(cd "$APP_DIR/../.." && pwd)"

# Xcode sets CONFIGURATION to "Debug"/"Release"; default to a debug Rust
# build for a standalone terminal run.
case "${CONFIGURATION:-Debug}" in
    Release) PROFILE=release ;;
    *) PROFILE=debug ;;
esac
CARGO_FLAGS=()
if [ "$PROFILE" = "release" ]; then
    CARGO_FLAGS+=(--release)
fi

echo "==> cargo build -p pi-core-ffi ($PROFILE)"
# `${arr[@]+"${arr[@]}"}` (not the plainer `"${arr[@]}"`) because macOS ships
# bash 3.2, where expanding an empty array under `set -u` is itself an
# "unbound variable" error — a bash 4+ idiosyncrasy this repo can't assume.
(cd "$ROOT" && cargo build -p pi-core-ffi "${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"}")

LIB_DIR="$ROOT/target/$PROFILE"
STATIC_LIB="$LIB_DIR/libpi_core_ffi.a"
DYLIB="$LIB_DIR/libpi_core_ffi.dylib"

GENERATED_DIR="$APP_DIR/Generated"
rm -rf "$GENERATED_DIR"
mkdir -p "$GENERATED_DIR"

echo "==> uniffi-bindgen generate"
(cd "$ROOT" && cargo run --quiet -p pi-core-ffi --bin uniffi-bindgen -- \
    generate --library "$DYLIB" --language swift --out-dir "$GENERATED_DIR")

# Xcode's `-create-xcframework -headers` treats the headers directory as a
# Clang module: it needs the generated header plus a module map, discovered
# implicitly only under the exact filename `module.modulemap` (not
# uniffi-bindgen's default `pi_core_ffiFFI.modulemap`).
HEADERS_DIR="$GENERATED_DIR/Headers"
mkdir -p "$HEADERS_DIR"
mv "$GENERATED_DIR/pi_core_ffiFFI.h" "$HEADERS_DIR/"
mv "$GENERATED_DIR/pi_core_ffiFFI.modulemap" "$HEADERS_DIR/module.modulemap"
# pi_core_ffi.swift is left in $GENERATED_DIR — added to the SwiftyPi Xcode
# target as a normal Swift source file (a project-referenced file picks up
# changes here automatically since this Run Script phase runs before Xcode's
# own Swift compilation phase).

XCFRAMEWORK="$APP_DIR/PiCoreFFI.xcframework"
rm -rf "$XCFRAMEWORK"
echo "==> xcodebuild -create-xcframework"
xcodebuild -create-xcframework \
    -library "$STATIC_LIB" -headers "$HEADERS_DIR" \
    -output "$XCFRAMEWORK"

echo "==> done: $XCFRAMEWORK"
