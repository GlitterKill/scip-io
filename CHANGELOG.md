# Changelog

All notable changes to **SCIP-IO** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Emit Python member metadata in its defining document when cross-file references
  cache the identity first. Preserve raw occurrences and existing metadata, and
  cover both file orders, same-file reads, locals, and unrelated attribute writes.

## [0.2.5] - 2026-09-13

### Fixed

- Preserve documentation and inheritance metadata for forward-referenced Python classes.
- Emit metadata for nonlocal annotations, nested/starred destructuring, loop targets,
  with aliases, exception aliases, assignment expressions, and pattern captures in
  managed `scip-python` 0.6.6. Generic binding repair retains emitted identities and
  ranges, including when hover documentation is unavailable.
- Keep ignored `_` destructuring and loop targets free of orphan metadata.
  Assignment-target repair intentionally corrects builtin-shadowing targets such as
  `for int in [1]` to the bound variable while retaining source ranges.
- Apply repairs to fresh and existing managed installs. Add raw-emitter regression
  checks for unique metadata, unchanged generic occurrences, missing documentation,
  builtin shadowing, and the separately unresolved invalid starred fixture.
  Qualification gates remain unchanged.

## [0.2.4] - 2026-09-12

### Fixed

- Accept Python `.pyi` stubs in explicit file manifests and infer their SCIP language.
- Repair managed `scip-python` 0.6.6 explicit file targets under hidden directories,
  slot attribute identities, and duplicated names in module-level annotated variables.
  Add real-emitter regression coverage, including inherited slots.

## [0.2.3] - 2026-09-11

### Fixed

- Repair the `scip-python` 0.6.6 emitter's wildcard-import symbol cache.
  Functions re-exported through `from module import *` now retain their own
  targets instead of pointing to the first function resolved from the import.
  Both fresh and existing managed npm installations receive the repair;
  custom indexer binaries remain caller-managed.
- Add regression coverage for existing-install repair and idempotence, plus
  a real-emitter check for re-exports, aliases, and repeated calls. All five
  reported Seaborn target mismatches are corrected in fresh raw SCIP output.

## [0.2.2] - 2026-09-10

### Fixed

- Normal Java, Kotlin, and Scala installations now select the published
  `scip-java-v0.12.3-scip-io.2` distribution automatically. It includes the
  Windows path repair, Kotlin 2.3.21 compiler compatibility, SemanticDB
  source-range fixes, and Gradle Kotlin DSL indexing.
- The compiler plugin is bundled in the CLI and Gradle plugin, so indexing
  no longer requires the unpublished local Maven snapshot or a
  `JAVA_TOOL_OPTIONS` override. Ordinary project dependencies resolve normally.
- The new package retains checksum verification and a separate versioned cache.
  Existing `.1` installations remain untouched. Compatibility is verified with
  Moshi on Windows, project Kotlin 2.3.21, and Gradle 9.5.1; other compiler
  combinations require separate qualification.

## [0.2.1] - 2026-09-09

### Fixed

- Default Java, Kotlin, and Scala installation selects the pinned
  `scip-java-v0.12.3-scip-io.1` path-serialization backport instead of reusing
  stock scip-java from an unversioned cache or PATH. Native and Linux backend
  installs verify SHA-256 hashes and publish a complete versioned cache while
  preserving old launchers. This release does not include the later Kotlin
  compiler compatibility patches.

## [0.2.0] - 2026-09-09

### Added

- Added documented upstream backport patches for Windows Gradle path escaping,
  Kotlin 2.3.21 compiler compatibility, SemanticDB source ranges, and Gradle
  Kotlin DSL script indexing, with regression fixtures, submission drafts,
  pinned local integration patches, and verification evidence. These are
  source patches for separately built indexers; SCIP-IO does not install the
  unpublished patched indexer or plugin builds automatically.

### Fixed

- CLI native indexing now honors configured indexer `binary` overrides before
  consulting managed installations. Relative paths resolve from the loaded
  config directory, language overrides take precedence over indexer-name
  overrides, and missing binaries fail without silently falling back.
- Text and JSON dry runs now show the same configured executable used by CLI
  indexing, with regression coverage for custom and missing binary paths.
- Use fixed-size byte chunks for UTF-16 process output decoding to satisfy the
  latest stable Clippy lint without changing decoding behavior.

## [0.1.9] - 2026-06-17

### Added

- Added explicit source-file indexing for both the CLI and GUI. `scip-io index`
  now accepts `--files` and `--files-from`, validates listed files under the
  selected repo root, runs only represented languages, filters SCIP documents to
  those exact files, and filters C/C++ compile databases before invoking
  `scip-clang`.

### Changed

- Consolidated internal indexer-runner helper inputs so source-file filtering
  stays threaded through project/config sharding and Python sharding without
  widening helper function signatures past the clippy release gate.

## [0.1.8] - 2026-06-06

### Added

- Added opt-in multi-`compile_commands.json` consumption for `scip-clang`.
  `--include-additional-configs` now discovers validated root and build-output
  compile databases, merges and deduplicates commands before indexing, reports
  selected/skipped database and coverage-delta counts in dry-run output, and
  preserves root-only `scip-clang` behavior when the option is not enabled.
- Added explicit CMake compile database generation for C/C++. The
  `--generate-cmake-compile-dbs` flag and `[cpp.cmake]` config can run
  configure-only CMake jobs with `CMAKE_EXPORT_COMPILE_COMMANDS=ON`, including
  an `llvm-broad` preset for broader LLVM target/project/runtime coverage, then
  feed those generated databases through the existing C/C++ merge path. On
  Windows, generation follows the WSL backend when `scip-clang` is WSL-backed so
  generated compile databases stay Linux-path compatible.
- Added `[cpp.coverage]` compile database curation for C/C++. Users can include
  or exclude discovered `compile_commands.json` paths and set `min_new_files` so
  large repositories skip low-gain duplicate databases while dry-run output
  reports per-database selection, skip reason, command counts, and file gain.
  Profiles that filter out every C/C++ compile database now fail as config
  errors instead of falling back to root-only indexing.

### Fixed

- Preserved split-index `Metadata.project_root` and
  `Metadata.text_document_encoding` values when merging SCIP artifacts, so the
  final `index.scip` keeps producer-side repository root and encoding metadata
  when all inputs agree.
- Added an authoritative-root merge API and routed CLI/GUI `index` merges
  through it so multi-language index output records the selected repository root
  even when nested project artifacts contributed child-root metadata.
- Repaired managed `scip-python` npm installs with the embedded Pyright
  wildcard-import assertion patch so large Python indexes such as LLVM do not
  fail during analysis when import metadata is stale.
- Preserved input-level SCIP `externalSymbols` while merging split artifacts so
  merged C/C++ indexes keep the external symbol metadata emitted by
  `scip-clang`.
- Planned Python files under dot-path directories such as `.ci/` and
  `.github/` as explicit shard targets even when the whole Python tree fits in
  one shard target, so `scip-python` does not skip hidden scan-scope files.

## [0.1.7] - 2026-06-02

### Added

- Added automatic nested project indexing for manifest/config-bearing child
  roots across TypeScript, JavaScript, Python, Rust, Go, Java, C#, Ruby,
  Kotlin, C/C++ compile databases, and Scala. Parent runs now exclude those
  nested roots and child SCIP document paths are prefixed before merge.
- Added partial-index reporting across CLI text output, CLI JSON output, and
  the GUI. When at least one language succeeds and another fails, SCIP-IO now
  publishes the successful output and reports the failed language count/details
  instead of presenting the run as a generic success or hiding usable output.

### Fixed

- Pruned parent SCIP documents owned by nested child roots before merging so
  root-bound indexers do not duplicate facts that are produced by separately
  scheduled child project runs.
- Tightened C/C++ nested-root promotion so only `compile_commands.json` creates
  an indexable C/C++ root; CMake, Makefile, Kbuild, and Kconfig evidence still
  detect C/C++ but no longer imply a runnable `scip-clang` child root.
- Improved shared TypeScript/JavaScript planning so nested TypeScript evidence
  without explicit root configs uses the JavaScript-style `--infer-tsconfig`
  invocation for the shared `scip-typescript` run.

## [0.1.6] - 2026-05-30

### Fixed

- Suppressed transient Windows console windows when the GUI launches child
  processes for indexer status checks, WSL/Docker probes, installs, indexing,
  and output reveal actions.
- Reused WSL/Docker backend probe results within a single GUI status refresh so
  opening the app or selecting a folder does not relaunch the same probe for
  every backend-capable indexer row.

## [0.1.5] - 2026-05-29

### Added

- Expanded language detection beyond manifest files. SCIP-IO now detects every
  supported language from source files, project config files, and build files,
  including Linux-style C/C++ evidence such as `Makefile`, `Kbuild`, and
  `Kconfig`, plus Rust `.rs` files without `Cargo.toml`.
- Added detector readiness metadata so the CLI and GUI can report when a
  detected language still needs indexer-specific setup such as
  `compile_commands.json` for `scip-clang` or `Cargo.toml`/`rust-project.json`
  for `rust-analyzer`.
- Added opt-in additional config discovery for indexing. The CLI now supports
  `index --include-additional-configs`, and the GUI exposes the same behavior
  with an Extra configs option. Supported multi-config inputs currently include
  root-level `tsconfig.json` and `tsconfig.*.json` files for
  `scip-typescript` and `.sln`, `.csproj`, or `.vbproj` files for
  `scip-dotnet`.
- Added automatic memory-bounded `scip-python` sharding for large Python
  projects. Repositories with more than 750 `.py`, `.pyi`, or `.pyw` files are
  indexed through bounded `--target-only` shards and merged back into the
  expected `python.scip` output without requiring `NODE_OPTIONS` heap tuning.
- Reduced `scip-python` shard startup overhead by using larger initial shard
  targets while preserving recursive OOM splitting as the memory-safety
  fallback.
- Added conservative parallel execution for small `scip-python` shards,
  per-shard timing logs, and local heap-limit split hints so repeat runs can
  pre-split targets that previously exceeded the Node heap.
- Packed loose Python files inside oversized directories into bounded file
  batches to avoid excessive one-file `scip-python` invocations in flat trees.
- Added protected temp-output execution for every indexer. The shared CLI/GUI
  runner now captures stdout/stderr, kills child processes on cancellation,
  normalizes and validates output before publishing `<language>.scip`, and logs
  elapsed time, output size, document, symbol, occurrence, compaction, retry,
  and shard telemetry.
- Added capability-gated cross-language sharding infrastructure. TypeScript,
  JavaScript, and C# can retry memory failures with safe project/config
  argument shards, C/C++ can chunk large `compile_commands.json` inputs, and
  Go/Rust/JVM/Ruby retain protected single-run behavior unless a safe upstream
  shard boundary is available.
- Sharded large project/config argument lists up front to avoid Windows command
  line length failures when repositories expose hundreds or thousands of
  `.csproj`/`.vbproj`/`.sln` or `tsconfig*` inputs.
- Added WSL/Docker execution backends for Windows users of `scip-ruby` and
  `scip-clang`. Native Windows installs remain marked unsupported because
  upstream publishes Linux/macOS assets only, but indexing can now prepare the
  Linux binary, translate backend paths, and publish normal `.scip` output
  through the protected runner.
- Added CLI/GUI status fields for native support, backend support, selected
  backend, and backend availability so Ruby/C/C++ Windows users see why native
  install is unavailable and which fallback can run.
- Added toolchain preflight and runtime environment injection for `scip-go` and
  `scip-java`. SCIP-IO now discovers Go and JDK/JVM installs from project
  config, environment variables, PATH, and common install locations, then
  injects PATH/JAVA_HOME only into child indexer processes.
- Added config-aware status reporting for selected execution backends and
  toolchain readiness in both the CLI and GUI.
- Added per-indexer argument overrides to the shared CLI/GUI runner path, so
  `.scip-io.toml` can supply complete commands for build-tool-specific JVM
  indexing cases such as custom Gradle targets or external SemanticDB
  targetroots.

### Fixed

- Compacted SCIP outputs after indexer runs and final merge/copy operations so
  duplicate documents, duplicate occurrences, and duplicate document symbols do
  not reach downstream consumers.
- Extended validation to fail on duplicate occurrence facts and duplicate
  document symbols, not just duplicate document paths.
- Repaired empty `Document.relative_path` values from `scip-python`
  single-file shards by mapping them back to the shard's repo-relative file
  path before merge compaction.
- Prevented failed non-Python indexers from replacing a previous successful
  `<language>.scip` with partial or invalid output.
- Fixed managed `.tar.gz` extraction on Windows for release archives that ship
  an `.exe` binary, including `scip-go`.
- Added C/C++ Linux-backend preflight checks that reject Windows drive-letter,
  Visual Studio, `cl.exe`, and `clang-cl.exe` compile commands before invoking
  `scip-clang` through WSL or Docker.
- Made WSL backend probing, path translation, permission fixups, and execution
  respect the configured `wsl_distro`.
- Corrected the managed `scip-ruby` release tag and invocation so SCIP-IO uses
  the upstream `--index-file <output> .` form instead of a nonexistent `index`
  subcommand.
- Corrected `scip-go` invocation so `index` is not passed as a package pattern,
  preventing large Go repos from producing an empty SCIP index.
- Added deterministic local Ruby gem metadata for app-only projects without a
  `.gemspec`, allowing `scip-ruby` to index applications as well as gems.
- Normalized extended Windows paths before WSL path translation and Docker
  bind mounts so project roots such as `\\?\F:\...` become backend-compatible
  paths.
- Allowed default-output indexers such as `scip-clang` to copy/remove the
  generated `index.scip` when Windows cannot rename it across drives.
- Updated the default Docker backend image to `ubuntu:24.04` so upstream Linux
  `scip-clang` binaries find a compatible glibc without custom image setup.
- Tightened WSL availability detection so SCIP-IO requires a runnable
  selected/default distro instead of treating `wsl.exe --status` as sufficient.
- Status commands now report invalid `.scip-io.toml` errors instead of silently
  falling back to default configuration.
- Avoided inferring `JAVA_HOME` from PATH-only Java shims while still allowing
  configured, environment, common-location, and macOS `.jdk/Contents/Home`
  homes to be injected for `scip-java`.
- Skipped generated `build`, `dist`, and `out` directories during language
  detection so build artifacts cannot make a project look like it contains an
  unrelated supported language.
- Corrected document-language normalization to prefer known source-file
  extensions over wrong non-empty indexer metadata, allowing mixed JVM outputs
  to label Kotlin and Scala documents correctly when `scip-java` reports a
  broader language.

## [0.1.4] - 2026-05-26

### Added

- Added GUI install/uninstall actions for each registered SCIP indexer, with
  uninstall limited to binaries managed in SCIP-IO's local cache.
- Added managed indexer update checks in Settings with per-indexer update
  actions and an Update All action when multiple managed installs have newer
  compatible versions available.
- Added CLI `install`, `uninstall`, and `update` commands. `update` reports
  installed indexer versions, opens an interactive terminal picker by default,
  and supports non-interactive `--lang` and `--all` update paths.
- Wired CLI monorepo scan controls: `detect --depth` now controls language
  evidence scan depth, `index --roots` indexes explicit sub-project roots, and
  `index --all-roots` discovers manifest/config-bearing project roots under the
  selected path.
- Managed indexer installs now resolve the latest compatible version at install
  time and record installed-version metadata for later update checks.

### Fixed

- The GUI now preflights indexer installation before launching any indexing
  process, so a first-run index can install a missing indexer and complete in
  the same operation.
- Repaired managed `scip-python` npm installs on Windows when the upstream
  bundle contains the `path.sep` regex bug that crashes under current Node
  versions.
- The GUI Kotlin row now shows that it is covered by `scip-java`; its
  install/uninstall action manages `scip-java`, and Kotlin indexing plans run
  the `scip-java` indexer instead of the non-standalone `scip-kotlin` plugin.
- Fixed the GUI "Back to Dashboard" action after successful indexing by
  clearing completed-run state before returning to the dashboard.
- Filled missing `Document.language` metadata after indexer runs and when
  merges can infer it from document paths, so TypeScript/JavaScript SCIP
  outputs and combined `index.scip` files preserve language information.
  Validation now warns when input indexes still contain documents without
  language metadata.
- Fixed Linux/macOS CI coverage for the Windows-only `scip-python` npm bundle
  repair by asserting that existing installs remain unchanged on non-Windows
  platforms while Windows still verifies the compatibility patch.

## [0.1.1] - 2026-04-06

### Fixed

- **CLI UX on Windows**: When `scip-io.exe` was launched by double-clicking
  from Explorer with no arguments, it printed a terse "GUI not yet
  implemented" message and exited immediately, causing the console window
  to flash and close so fast it looked like a crash. The CLI now prints a
  help banner with a link to the graphical installer download, shows full
  clap help, and — when it detects it was launched from Explorer (via
  `GetConsoleProcessList` returning 1) — waits for the user to press Enter
  before exiting so the output is actually readable. The `gui` subcommand
  similarly prints the GUI download information instead of a placeholder.

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
- **Language detection** that ignores `node_modules`, `target`, `vendor`,
  `venv`, and other noise directories.
- **Deterministic SCIP merge** that combines per-language `.scip` files into
  a single `index.scip`, with document de-duplication.
- **SCIP index validation** reporting document, symbol, occurrence, and
  language counts.
- **Configurable per-project defaults** via optional `.scip-io.toml`.
- **Cross-platform release artifacts** — CLI archives and GUI installers for
  Windows, macOS (Intel + Apple Silicon), and Linux via GitHub Actions.
- **One-line install scripts** for the CLI on Linux/macOS (`install.sh`) and
  Windows (`install.ps1`).

[Unreleased]: https://github.com/GlitterKill/scip-io/compare/v0.2.4...HEAD
[0.2.4]: https://github.com/GlitterKill/scip-io/compare/v0.2.3...v0.2.4
[0.2.3]: https://github.com/GlitterKill/scip-io/compare/v0.2.2...v0.2.3
[0.2.2]: https://github.com/GlitterKill/scip-io/compare/v0.2.1...v0.2.2
[0.2.1]: https://github.com/GlitterKill/scip-io/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/GlitterKill/scip-io/compare/v0.1.9...v0.2.0
[0.1.9]: https://github.com/GlitterKill/scip-io/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/GlitterKill/scip-io/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/GlitterKill/scip-io/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/GlitterKill/scip-io/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/GlitterKill/scip-io/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/GlitterKill/scip-io/compare/v0.1.3...v0.1.4
[0.1.1]: https://github.com/GlitterKill/scip-io/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/GlitterKill/scip-io/releases/tag/v0.1.0
