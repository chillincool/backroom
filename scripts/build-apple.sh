#!/usr/bin/env bash
# Builds backroom-ffi for Apple platforms and refreshes the generated files
# in apple/BackroomCore (both git-ignored):
#   BackroomFFI.xcframework             static library + C header
#   Sources/BackroomCore/backroom_ffi.swift  Swift bindings
#
#   build-apple.sh              this Mac's architecture, release
#   build-apple.sh --universal  Intel + Apple Silicon (needs both rustup targets)
#   build-apple.sh --debug      debug build (faster to compile)
#
# Needs Xcode (xcodebuild), not just the Command Line Tools.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
PKG=$ROOT/apple/BackroomCore
OUT=$ROOT/target/apple
LIB=libbackroom_ffi

profile=release
universal=false
for arg in "$@"; do
	case $arg in
		--universal) universal=true ;;
		--debug) profile=debug ;;
		*)
			echo "unknown option: $arg" >&2
			exit 2
			;;
	esac
done

log() { printf '\n==> %s\n' "$*"; }

if ! xcodebuild -version > /dev/null 2>&1; then
	echo "xcodebuild not found: install Xcode and run 'sudo xcode-select -s /Applications/Xcode.app'" >&2
	exit 1
fi

case $(uname -m) in
	x86_64) host=x86_64-apple-darwin ;;
	arm64) host=aarch64-apple-darwin ;;
	*)
		echo "unsupported architecture $(uname -m)" >&2
		exit 1
		;;
esac
targets=("$host")
if $universal; then
	targets=(x86_64-apple-darwin aarch64-apple-darwin)
fi

# Match Package.swift's `platforms`; without this, C/assembly in ring is
# built for the running macOS and the linker warns (and older Macs may fail).
export MACOSX_DEPLOYMENT_TARGET=14.0

cargo_flags=(--locked -p backroom-ffi)
[[ $profile == release ]] && cargo_flags+=(--release)

libs=()
for target in "${targets[@]}"; do
	log "Building backroom-ffi for $target ($profile)"
	rustup target add "$target" > /dev/null 2>&1 || true
	cargo build "${cargo_flags[@]}" --target "$target"
	libs+=("$ROOT/target/$target/$profile/$LIB.a")
done

rm -rf "$OUT"
mkdir -p "$OUT/headers"
if ((${#libs[@]} > 1)); then
	lipo -create "${libs[@]}" -output "$OUT/$LIB.a"
else
	cp "${libs[0]}" "$OUT/$LIB.a"
fi

log "Generating Swift bindings"
# The interface is the same for every architecture; read it from the first.
cargo run --locked -q -p backroom-ffi --features bindgen --bin uniffi-bindgen -- \
	generate --library "$ROOT/target/${targets[0]}/$profile/$LIB.dylib" \
	--language swift --out-dir "$OUT/bindings"
cp "$OUT/bindings/backroom_ffiFFI.h" "$OUT/headers/"
cp "$OUT/bindings/backroom_ffiFFI.modulemap" "$OUT/headers/module.modulemap"

log "Creating BackroomFFI.xcframework"
rm -rf "$PKG/BackroomFFI.xcframework"
xcodebuild -create-xcframework \
	-library "$OUT/$LIB.a" -headers "$OUT/headers" \
	-output "$PKG/BackroomFFI.xcframework"
cp "$OUT/bindings/backroom_ffi.swift" "$PKG/Sources/BackroomCore/backroom_ffi.swift"

log "Done: $PKG"
