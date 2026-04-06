# SCIP-IO

SCIP Index Orchestrator — a Rust CLI that detects project languages, downloads/manages SCIP indexer binaries, runs them, and merges the resulting `.scip` index files.

## Build & Test

```sh
cargo build
cargo test
cargo run -- detect
cargo run -- index --lang ts
cargo run -- status
cargo run -- merge a.scip b.scip -o merged.scip
```

## Architecture

```
src/
  main.rs           — CLI entry point, tokio runtime setup
  cli/              — Clap command definitions and handlers
    mod.rs          — Cli struct, Command enum, arg structs
    detect.rs       — `scip-io detect`
    index.rs        — `scip-io index`
    status.rs       — `scip-io status`
    merge.rs        — `scip-io merge`
  detect/           — Language detection from manifest files
    mod.rs          — scan_languages() walks project tree
    languages.rs    — LanguageKind enum, manifest matching
  indexer/          — Indexer binary management
    mod.rs          — IndexerEntry struct, install_dir()
    registry.rs     — Static registry of known SCIP indexers
    download.rs     — GitHub release download logic
    runner.rs       — Invokes indexer binary as subprocess
  merge/            — SCIP protobuf merging
    mod.rs          — merge_scip_files(), merge_document()
  config/           — Project config (.scip-io.toml)
    mod.rs          — ProjectConfig, IndexerOverride
```

## Conventions

- Error handling: `anyhow::Result` for CLI, `thiserror` for library errors
- Async: tokio runtime, reqwest for HTTP
- SCIP: uses the `scip` crate (protobuf types) + `protobuf` for serialization
- Indexer binaries stored in `dirs::data_local_dir()/scip-io/bin/`
- Config: optional `.scip-io.toml` in project root

## Key Dependencies

- `clap` — CLI parsing with derive
- `scip` + `protobuf` — SCIP index reading/writing/merging
- `tokio` + `reqwest` — async runtime and HTTP for downloads
- `walkdir` — filesystem traversal for language detection
- `indicatif` + `console` — progress bars and terminal styling
