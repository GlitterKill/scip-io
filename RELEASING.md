# Releasing SCIP-IO

This document describes how to cut a new release of SCIP-IO. Releases are
fully automated through GitHub Actions — the human steps are about
version bumping, changelog hygiene, and pushing a tag.

## Prerequisites

- You have push access to `GlitterKill/scip-io`.
- You have `git` and `cargo` locally.
- Your working tree is clean (no uncommitted changes).

## Versioning

SCIP-IO uses [Semantic Versioning](https://semver.org/):

- **MAJOR** (`v1.0.0` → `v2.0.0`) — incompatible CLI flags, config schema
  changes, or SCIP output format changes users must adapt to.
- **MINOR** (`v0.1.0` → `v0.2.0`) — new commands, new supported languages,
  new flags. Backwards-compatible.
- **PATCH** (`v0.1.0` → `v0.1.1`) — bug fixes, indexer version bumps,
  documentation fixes.

While on `0.x`, **minor bumps may include breaking changes**. Tag `1.0.0`
once the CLI surface is considered stable.

## Release checklist

### 1. Verify green main

```sh
git checkout main
git pull
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test
```

The GitHub `CI` workflow should also be green on the latest `main` commit.

### 2. Bump the version

Edit `Cargo.toml` at the workspace root:

```toml
[workspace.package]
version = "0.2.0"   # ← bump this
```

Then regenerate the lockfile:

```sh
cargo check --workspace
```

### 3. Update the changelog

Edit `CHANGELOG.md`:

- Move entries from `## [Unreleased]` into a new `## [0.2.0] - YYYY-MM-DD`
  section.
- Make sure every user-visible change is documented under `### Added`,
  `### Changed`, `### Fixed`, `### Deprecated`, or `### Removed`.
- Update the link references at the bottom:
  ```
  [Unreleased]: https://github.com/GlitterKill/scip-io/compare/v0.2.0...HEAD
  [0.2.0]: https://github.com/GlitterKill/scip-io/compare/v0.1.0...v0.2.0
  [0.1.0]: https://github.com/GlitterKill/scip-io/releases/tag/v0.1.0
  ```

### 4. Commit and tag

```sh
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "Release v0.2.0"
git tag -a v0.2.0 -m "SCIP-IO v0.2.0"
git push origin main
git push origin v0.2.0
```

### 5. Watch the release workflow

Open <https://github.com/GlitterKill/scip-io/actions> and watch the
`Release` workflow. It will:

1. Create a **draft** GitHub Release for the tag.
2. Build the CLI for Windows x64, macOS x64, macOS ARM64, and Linux x64.
3. Build the Tauri GUI for Windows, macOS x64, macOS ARM64, and Linux.
4. Upload all artifacts to the draft release.
5. Generate `SHA256SUMS.txt` and upload install scripts.
6. Publish the release (remove the draft flag).

The whole pipeline takes ~15–25 minutes depending on runner availability.

### 6. Verify the release

Once published, check <https://github.com/GlitterKill/scip-io/releases>:

- [ ] All CLI archives are present (`scip-io-vX.Y.Z-*.{tar.gz,zip}`).
- [ ] All GUI installers are present (`SCIP-IO_*-setup.exe`, `*.msi`,
      `*.dmg`, `*.deb`, `*.AppImage`).
- [ ] `SHA256SUMS.txt`, `install.sh`, and `install.ps1` are attached.
- [ ] The release body has the download table and install instructions.
- [ ] The quick-install scripts work end to end:
  ```sh
  # Linux/macOS
  curl -LsSf https://github.com/GlitterKill/scip-io/releases/latest/download/install.sh | sh
  scip-io --version
  ```
  ```powershell
  # Windows
  irm https://github.com/GlitterKill/scip-io/releases/latest/download/install.ps1 | iex
  scip-io --version
  ```

### 7. Announce

- Update any downstream references (documentation sites, READMEs, etc.).
- Post release notes anywhere the project is discussed.

## Hotfix releases

For critical fixes on a released version:

1. Create a branch from the tag: `git switch -c hotfix/v0.2.1 v0.2.0`
2. Cherry-pick the fix commit(s).
3. Bump patch version in `Cargo.toml`.
4. Update `CHANGELOG.md`.
5. Tag `v0.2.1` on the hotfix branch and push.
6. Merge the hotfix branch back into `main`.

## Re-running a failed release

If the workflow fails partway through:

1. Fix the issue on `main`.
2. Delete the failed tag locally and remotely:
   ```sh
   git tag -d v0.2.0
   git push origin :refs/tags/v0.2.0
   ```
3. Delete the draft release on GitHub.
4. Re-tag and re-push.

Alternatively, use **workflow_dispatch** from the Actions tab to retry a
specific tag without re-tagging.

## Code signing (future)

Currently, release binaries are **unsigned**. On Windows, users will see a
SmartScreen warning; on macOS, Gatekeeper blocks the app unless the user
right-clicks → Open. To enable signing:

- **Windows**: purchase a code signing certificate (Sectigo, DigiCert) and
  add `WINDOWS_CERTIFICATE` + `WINDOWS_CERTIFICATE_PASSWORD` GitHub secrets.
  Update `tauri.conf.json` and the `release.yml` to sign the MSI/EXE.
- **macOS**: enroll in the Apple Developer Program ($99/yr), generate a
  Developer ID certificate, and add `APPLE_CERTIFICATE`,
  `APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD`, and
  `APPLE_TEAM_ID` secrets. `tauri-action` will pick these up
  automatically and notarize the DMG.

Until then, the documented install scripts and the README's troubleshooting
section cover the unsigned-binary workflow for end users.
