# AmpRtspClient for iOS

This directory is not part of upstream Retina. It holds `amp-rtsp-client`, a
small Rust library that wraps Retina behind a C interface for the native AMP iOS
app, and the scripts that publish it as a prebuilt `AmpRtspClient.xcframework`.
Everything lives under `ios/` plus two workflow files and a small change to
`.github/workflows/check-license.py` (files under `ios/` carry their own
copyright holder), so the fork rebases onto upstream Retina with few conflicts.

`amp-rtsp-client` is Copyright (C) Mike Olson and, like Retina, licensed under
either the MIT license or the Apache License 2.0 (`amp-rtsp-client/LICENSE-MIT`,
`amp-rtsp-client/LICENSE-APACHE`).

- `amp-rtsp-client/`: the crate. It depends on Retina by path (`../..`), so a
  release uses exactly the Retina source in the same commit. It has its own
  `Cargo.lock` and is not a member of Retina's workspace.
- `amp-rtsp-client/include/`: `AmpRtspClient.h` and `module.modulemap`, the C
  interface the app imports as the `AmpRtspClient` module.
- `amp-rtsp-client/rust-toolchain.toml`: the pinned Rust toolchain and the two
  iOS targets.
- `amp-rtsp-client/licenses/`: license texts for crates that do not ship one.
- `rtsp-client-notices.py`: fails when a locked crate lacks a permissive
  license, and writes `Retina-LICENSE.txt` (amp-rtsp-client's own notice, then
  every crate linked into the library) and `RustStandardLibrary-LICENSE.txt`. A
  license expression that does not parse completely, or has a name without SPDX
  identifier syntax, counts as not permissive. Names are not checked against the
  SPDX license list; an unknown name is harmless because only allowlisted names
  make an expression permissive. `test_rtsp_client_notices.py` tests that check.
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
  which an app linking the library must ship. The first one carries
  amp-rtsp-client's own copyright and license texts as well as Retina's and
  every dependency's.
- `manifest.json` with the crate version and license, Retina version, source
  commit, Rust toolchain, Xcode and SDK versions, targets, deployment target,
  the notice files to ship and the SHA-256 of every file

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

## Releasing

Releases come from tags `amp-rtsp-client-v<version>`, which stay apart from
upstream Retina's `v*` tags. The fork's `main` carries this directory on top of
upstream, and `ios-xcframework` is the working branch for it. Immutable releases
are enabled for the repository, so a published release's assets and tag cannot
change and consumers can rely on the SHA-256 they pin.

1. Check for uncommitted changes and fetch tags:

   ```sh
   git status
   git fetch --tags origin
   ```

   To pick up a new Retina, rebase `ios-xcframework` onto the upstream release
   first, then force-push `ios-xcframework` and `main` with
   `--force-with-lease`. Earlier release tags keep their commits.

2. Run the full checks: the Rust checks and parser tests above, plus
   `actionlint` on the workflows and `shellcheck ios/build-xcframework.sh`.
   Build the zip on a Mac with `ios/build-xcframework.sh`.

3. If the release changes the version, update `version` in
   `amp-rtsp-client/Cargo.toml`, run `cargo update -p amp-rtsp-client` in that
   directory, and commit the bump separately as
   `chore(ios): Bump amp-rtsp-client to <version>`.

4. Push `ios-xcframework` and wait for the `iOS xcframework CI` workflow to
   pass. Then fast-forward `main` to it and push `main`:

   ```sh
   git push origin ios-xcframework
   git switch main && git merge --ff-only ios-xcframework && git push origin main
   ```

5. Create and push the annotated release tag:

   ```sh
   git tag -a amp-rtsp-client-v<version> -m "amp-rtsp-client <version>"
   git push origin amp-rtsp-client-v<version>
   ```

6. Watch the tag-triggered `iOS xcframework publish` workflow. It runs the
   checks, builds the zip on a macOS runner with a pinned Xcode, attests its
   build provenance, creates a draft release with the zip and `SHA256SUMS`, and
   checks the uploaded assets' digests:

   ```sh
   gh run list --workflow ios-xcframework-publish.yml --limit 1
   gh run watch <run-id> --exit-status
   ```

7. If the workflow fails, fix `main`, delete the failed local and remote tag
   (only while no release for it has been published), retag the fixed commit and
   push the tag again. The workflow only creates releases: if a release for the
   tag already exists, draft or published, it fails without touching it, so
   delete a leftover draft by hand before rerunning.

8. Download the draft's assets and verify them:

   ```sh
   gh release download amp-rtsp-client-v<version> --dir /path/to/scratch
   shasum -a 256 -c SHA256SUMS
   gh attestation verify AmpRtspClient-<version>.xcframework.zip --repo mwolson/retina \
       --signer-workflow mwolson/retina/.github/workflows/ios-xcframework-publish.yml
   ```

9. Review the commits since the previous release tag and replace the generated
   draft notes. Start with a short summary, group related changes under
   descriptive headings, put user-visible changes first and maintenance
   afterward, and keep a "Full Changelog" compare link when there is a previous
   release. Leave verification steps and check commands out of the public notes.
   Then publish:

   ```sh
   gh release edit amp-rtsp-client-v<version> --notes-file notes.md --draft=false
   gh release view amp-rtsp-client-v<version> --json isImmutable
   ```

Consumers pin the release URL and SHA-256, and verify provenance with the
`gh attestation verify` command above.

`.github/workflows/ios-xcframework-ci.yml` runs the same checks and build for
pushes and pull requests, and uploads the zip as a workflow artifact only.
