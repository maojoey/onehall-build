# OneHall public client builds

This repository contains a sanitized source snapshot of OneHall, a desktop
workspace derived from [Codeg](https://github.com/spacering-net/codeg).
Upstream and bundled third-party licenses and attribution are retained.
See LICENSE and the vendored license files.

The development repository, its history, internal guides, device credentials,
deployment configuration and signing private keys are not included.
`public-source.json` records the source revision, version and SHA-256 of every
exported file. The public update endpoint and verification public key are
intentional parts of the client and are not secrets.

Run **Actions → OneHall cloud desktop → Run workflow** on `main` to build:

- macOS Intel: `macos-15-intel`, x86_64-apple-darwin
- macOS Apple Silicon: `macos-15`, aarch64-apple-darwin
- Windows x64: `windows-2022`, x86_64-pc-windows-msvc

Only standard GitHub-hosted runners are used. Workflow artifacts expire after
one day; download them promptly. Artifact storage has a separate quota from
compute. There are no private-repository checkout tokens, signing secrets or
automatic deployments in this workflow. Pull requests do not trigger builds.

Artifacts are **unsigned update preparation inputs**, not authorized updates.
The maintainer verifies the run, public snapshot, platform, version, inventory
and hashes, then signs on a private machine and publishes to the existing
authenticated update service. OS publisher signing/notarization is separate.
A successful cloud build does not install or restart any existing client.

To build manually: install Node 24, pnpm from packageManager, stable Rust and
the native SDK, run `pnpm install --frozen-lockfile`, then
`node scripts/build-hall-cloud.mjs --platform mac-arm64` (or mac-x64/win-x64).
Manual execution needs GITHUB_SHA (the current public commit) and GITHUB_RUN_ID
(a numeric provenance identifier), as supplied automatically by Actions.
