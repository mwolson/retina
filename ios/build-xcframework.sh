#!/bin/bash
# Builds amp-rtsp-client for iOS devices and the Simulator, assembles
# AmpRtspClient.xcframework, and packages it with its license notices and a
# manifest into a reproducible zip plus SHA256SUMS.
#
# Usage: ios/build-xcframework.sh [OUT_DIR]
#
# OUT_DIR defaults to ios/build/dist. The Rust toolchain comes from
# amp-rtsp-client/rust-toolchain.toml through rustup. Needs macOS with an
# Xcode that has the iOS 26 SDK.
set -euo pipefail

IOS_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(cd "$IOS_DIR/.." && pwd)"
CRATE_DIR="$IOS_DIR/amp-rtsp-client"
OUT_DIR="${1:-$IOS_DIR/build/dist}"
TARGET_DIR="$IOS_DIR/build/target"
STAGE_DIR="$IOS_DIR/build/stage"
LIBRARY=libamp_rtsp_client.a
TARGETS=(aarch64-apple-ios aarch64-apple-ios-sim)
DEPLOYMENT_TARGET=26.0

fail() {
    echo "error: build-xcframework: $*" >&2
    exit 1
}

for tool in git python3 rustup xcodebuild xcrun; do
    command -v "$tool" >/dev/null || fail "missing $tool"
done
[[ "$(uname -s)" == Darwin ]] || fail "needs macOS with Xcode"

# Let rust-toolchain.toml pick the toolchain, and keep host settings that
# Xcode or a parent build may export away from cargo.
unset RUSTUP_TOOLCHAIN RUSTC RUSTFLAGS CARGO_BUILD_RUSTFLAGS SDKROOT
export IPHONEOS_DEPLOYMENT_TARGET="$DEPLOYMENT_TARGET"

cd "$CRATE_DIR"
# Installs the pinned toolchain and its iOS targets when missing, then puts
# that toolchain first on PATH so a cargo from Homebrew or elsewhere is unused.
rustup toolchain install >/dev/null
toolchain="$(rustup show active-toolchain | awk '{print $1}')"
RUSTC="$(rustup which rustc)"
export RUSTC
PATH="$(dirname "$RUSTC"):$PATH"
rustc_version="$(rustc --version)"
crate_version="$(cargo metadata --locked --no-deps --format-version 1 |
    python3 -c 'import json, sys; print(json.load(sys.stdin)["packages"][0]["version"])')"
retina_version="$(cargo metadata --locked --format-version 1 |
    python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "retina"))')"
source_commit="$(git -C "$REPO_DIR" rev-parse HEAD)"
if [[ -n "$(git -C "$REPO_DIR" status --porcelain)" ]]; then
    source_commit="$source_commit-dirty"
fi
xcode_version="$(xcodebuild -version | paste -sd ' ' -)"
ios_sdk="$(xcrun --sdk iphoneos --show-sdk-version)"
case "$ios_sdk" in
    2[6-9].* | [3-9][0-9].*) ;;
    *) fail "iOS SDK $ios_sdk is older than 26.0; select a newer Xcode" ;;
esac

# Keep checkout and cargo home paths out of the libraries so builds from
# different directories produce the same bytes.
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
export CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=$REPO_DIR=/retina"$'\x1f'"--remap-path-prefix=$cargo_home=/cargo"

target_args=()
for target in "${TARGETS[@]}"; do
    target_args+=(--target "$target")
done
cargo build --locked --release --target-dir "$TARGET_DIR" "${target_args[@]}"

rm -rf "$STAGE_DIR"
mkdir -p "$STAGE_DIR" "$OUT_DIR"

# Fails on a crate without a permissive license, then writes the notices.
python3 "$IOS_DIR/rtsp-client-notices.py" --out "$STAGE_DIR/licenses"

device_lib="$TARGET_DIR/aarch64-apple-ios/release/$LIBRARY"
simulator_lib="$TARGET_DIR/aarch64-apple-ios-sim/release/$LIBRARY"
# rustc's LLVM is newer than Xcode's, so nm must skip the embedded bitcode.
for lib in "$device_lib" "$simulator_lib"; do
    symbols="$(xcrun nm --no-llvm-bc -gU "$lib")"
    for symbol in _amp_rtsp_client_start _amp_rtsp_client_stop; do
        grep -q " T $symbol\$" <<<"$symbols" || fail "$lib does not export $symbol"
    done
done
xcodebuild -create-xcframework \
    -library "$device_lib" -headers "$CRATE_DIR/include" \
    -library "$simulator_lib" -headers "$CRATE_DIR/include" \
    -output "$STAGE_DIR/AmpRtspClient.xcframework" >/dev/null

zip_name="AmpRtspClient-$crate_version.xcframework.zip"
CRATE_VERSION="$crate_version" DEPLOYMENT_TARGET="$DEPLOYMENT_TARGET" IOS_SDK="$ios_sdk" \
    RETINA_VERSION="$retina_version" RUSTC_VERSION="$rustc_version" RUST_TOOLCHAIN="$toolchain" \
    SOURCE_COMMIT="$source_commit" TARGETS="${TARGETS[*]}" XCODE_VERSION="$xcode_version" \
    python3 - "$STAGE_DIR" "$OUT_DIR" "$zip_name" <<'PY'
import hashlib, json, os, plistlib, sys, zipfile
from pathlib import Path

stage, out, zip_name = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


# xcodebuild lists the slices in no fixed order; sort them.
xcframework = stage / "AmpRtspClient.xcframework"
info_path = xcframework / "Info.plist"
info = plistlib.loads(info_path.read_bytes())
info["AvailableLibraries"].sort(key=lambda library: library["LibraryIdentifier"])
info_path.write_bytes(plistlib.dumps(info, sort_keys=True))

manifest = {
    "crate": "amp-rtsp-client",
    "crate_version": os.environ["CRATE_VERSION"],
    "files": {
        str(path.relative_to(stage)): sha256(path)
        for path in sorted(xcframework.rglob("*")) + sorted((stage / "licenses").rglob("*"))
        if path.is_file()
    },
    "ios_deployment_target": os.environ["DEPLOYMENT_TARGET"],
    "ios_sdk": os.environ["IOS_SDK"],
    "retina_version": os.environ["RETINA_VERSION"],
    "rustc": os.environ["RUSTC_VERSION"],
    "rust_toolchain": os.environ["RUST_TOOLCHAIN"],
    "source_commit": os.environ["SOURCE_COMMIT"],
    "targets": os.environ["TARGETS"].split(),
    "xcode": os.environ["XCODE_VERSION"],
    "xcframework": xcframework.name,
}
(stage / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")

# Fixed timestamps, permissions and order, so the same inputs give the same zip.
zip_path = out / zip_name
with zipfile.ZipFile(zip_path, "w") as archive:
    for path in sorted(stage.rglob("*"), key=lambda p: p.relative_to(stage).as_posix()):
        name = path.relative_to(stage).as_posix()
        info = zipfile.ZipInfo(name + "/" if path.is_dir() else name, date_time=(1980, 1, 1, 0, 0, 0))
        info.create_system = 3
        if path.is_dir():
            info.external_attr = (0o40755 << 16) | 0x10
            archive.writestr(info, b"")
        else:
            info.external_attr = 0o100644 << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, path.read_bytes(), compresslevel=9)
(out / "SHA256SUMS").write_text(f"{sha256(zip_path)}  {zip_name}\n")
PY

echo "Built $OUT_DIR/$zip_name"
cat "$OUT_DIR/SHA256SUMS"
