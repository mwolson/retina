# AmpRtspClient for iOS

This directory is not part of upstream Retina. It holds `amp-rtsp-client`, a
small Rust library that wraps Retina behind a C interface for the native AMP
iOS app, and the scripts that publish it as a prebuilt
`AmpRtspClient.xcframework`. Everything lives under `ios/` plus two workflow
files, so the fork rebases onto upstream Retina without conflicts.

- `amp-rtsp-client/`: the crate. It depends on Retina by path (`../..`), so a
  release uses exactly the Retina source in the same commit. It has its own
  `Cargo.lock` and is not a member of Retina's workspace.
- `amp-rtsp-client/include/`: `AmpRtspClient.h` and `module.modulemap`, the C
  interface the app imports as the `AmpRtspClient` module.
- `amp-rtsp-client/rust-toolchain.toml`: the pinned Rust toolchain and the two
  iOS targets.
- `amp-rtsp-client/licenses/`: license texts for crates that do not ship one.
- `rtsp-client-notices.py`: fails when a locked crate lacks a permissive
  license, and writes `Retina-LICENSE.txt` (every crate linked into the
  library) and `RustStandardLibrary-LICENSE.txt`. A license expression that
  does not parse completely counts as not permissive.
  `test_rtsp_client_notices.py` tests that check.
- `build-xcframework.sh`: builds the release zip on macOS.

## Build locally

Needs macOS, an Xcode with the iOS 26 SDK or newer, rustup and Python 3:

```bash
ios/build-xcframework.sh            # writes ios/build/dist/
```

The script builds `aarch64-apple-ios` and `aarch64-apple-ios-sim` with
`cargo build --locked --release` and `IPHONEOS_DEPLOYMENT_TARGET=26.0`, checks
that both libraries export the C entry points, and writes
`AmpRtspClient-<version>.xcframework.zip` and `SHA256SUMS`. The zip contains:

- `AmpRtspClient.xcframework/` with the static library and headers per slice
- `licenses/Retina-LICENSE.txt` and `licenses/RustStandardLibrary-LICENSE.txt`,
  which an app linking the library must ship
- `manifest.json` with the crate and Retina versions, source commit, Rust
  toolchain, Xcode and SDK versions, targets, deployment target and the SHA-256
  of every file

Two builds of the same commit from the same checkout path, with the same
toolchain and Xcode, give a byte-identical zip: paths are remapped out of the
libraries, the xcframework's slice list is sorted, and the zip has fixed
timestamps, permissions and entry order. A different checkout path changes the
static libraries, because Cargo hashes the absolute path of the out-of-workspace
`retina` path dependency into crate metadata and so into symbol names. Release
builds always run from the runner's fixed checkout path.

Rust checks run anywhere:

```bash
cd ios/amp-rtsp-client
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 ../test_rtsp_client_notices.py
```

## Releases

1. Bump `version` in `amp-rtsp-client/Cargo.toml` (and rebase onto upstream
   Retina first if the release should pick up a new Retina).
2. Push a tag `amp-rtsp-client-v<version>` on that commit. The tag prefix keeps
   these tags apart from upstream Retina's `v*` tags.
3. `.github/workflows/ios-xcframework-publish.yml` runs the checks, builds the
   zip on a macOS runner with a pinned Xcode, attests its build provenance, and
   attaches the zip and `SHA256SUMS` to a draft release.
4. Review the draft and publish it by hand.

Consumers pin the release URL and SHA-256, and can verify provenance with:

```bash
gh attestation verify AmpRtspClient-<version>.xcframework.zip --repo mwolson/retina \
    --signer-workflow mwolson/retina/.github/workflows/ios-xcframework-publish.yml
```

`.github/workflows/ios-xcframework-ci.yml` runs the same checks and build for
pushes and pull requests, and uploads the zip as a workflow artifact only.
