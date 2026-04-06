# Changelog

All notable changes to **SCIP-IO** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-04-06

Initial release.

### Added

- **CLI (`scip-io`)** with commands: `detect`, `index`, `status`, `merge`,
  `validate`, `clean`, `gui`, and `update-registry`.
- **Tauri 2 GUI (`SCIP-IO`)** with a dark cyberpunk-corporate theme, custom
  titlebar with working minimize/maximize/close controls, pipeline progress
  view, real-time per-language stats, and an "Open Output Location" button
  that reveals the merged index in the system file explorer.
- **11 supported languages** across 9 different SCIP indexers:
  - TypeScript, JavaScript (`scip-typescript`, npm)
  - Python (`scip-python`, npm)
  - Rust (`rust-analyzer`, GitHub gz/zip)
  - Go (`scip-go`, GitHub tar.gz)
  - Java, Scala (`scip-java`, Coursier launcher)
  - Kotlin (via `scip-java` compiler plugin)
  - C# (`scip-dotnet`, `dotnet tool install`)
  - Ruby (`scip-ruby`, GitHub release)
  - C / C++ (`scip-clang`, GitHub release)
- **Multi-method indexer installation** — GitHub binary, gzipped binary,
  tarball, zip, Coursier launcher script, npm package, `dotnet tool`, plus
  system `PATH` detection for pre-installed tools.
- **Manifest-driven language detection** that ignores `node_modules`,
  `target`, `vendor`, `venv`, and other noise directories.
- **Deterministic SCIP merge** that combines per-language `.scip` files into
  a single `index.scip`, with document de-duplication.
- **SCIP index validation** reporting document, symbol, occurrence, and
  language counts.
- **Configurable per-project defaults** via optional `.scip-io.toml`.
- **Cross-platform release artifacts** — CLI archives and GUI installers for
  Windows, macOS (Intel + Apple Silicon), and Linux via GitHub Actions.
- **One-line install scripts** for the CLI on Linux/macOS (`install.sh`) and
  Windows (`install.ps1`).

[Unreleased]: https://github.com/GlitterKill/scip-io/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/GlitterKill/scip-io/releases/tag/v0.1.0
