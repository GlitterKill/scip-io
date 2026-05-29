# Windows Linux Indexer Backends Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep `scip-ruby` and `scip-clang` explicitly unsupported as native Windows indexers while allowing Windows users to generate real `.scip` files by running the upstream Linux binaries through WSL or Docker.

**Architecture:** Separate "native install support" from "execution backend support" in `scip-io-core`. The existing protected runner remains the only artifact publication path; backend wrappers only translate paths, prepare Linux binaries, and execute the same indexer arguments inside WSL or Docker. WSL is the default automatic Windows fallback because it preserves local filesystem access and lets users install matching build dependencies; Docker is supported as an explicit or automatic fallback when available, but C/C++ requires a Linux-compatible `compile_commands.json` and toolchain inside the selected backend.

**Tech Stack:** Rust, Tokio process execution, Windows `wsl.exe`, Docker CLI, existing SCIP normalization/compaction/validation helpers, Tauri command payloads, Clap CLI.

---

## Design Decisions

**Backend selection order on Windows**

- Native remains selected for all indexers except `scip-ruby` and `scip-clang`.
- For `scip-ruby` and `scip-clang` on Windows, `native` reports unsupported unless the user supplies an explicit custom binary override.
- `auto` selects WSL first, then Docker. WSL is lower-friction for local repositories and dependency reuse.
- Users can force `wsl`, `docker`, `native`, or `disabled` through `.scip-io.toml`; CLI flags are optional, not required.

**WSL vs Docker tradeoff**

- WSL pros: simpler path translation with `wslpath`, lower runtime overhead, easier to install compilers/Ruby deps once, better fit for projects already developed in WSL.
- WSL cons: depends on a user-managed distro; indexing from `/mnt/<drive>` can be slower than inside the distro filesystem.
- Docker pros: reproducible runtime boundary, easier CI story, no permanent WSL distro mutation.
- Docker cons: C/C++ compile commands often need matching compilers/headers inside the image; bind-mounted Windows paths must be rewritten; Docker Desktop must be running.

**C/C++ correctness guard**

- Do not claim Docker/WSL makes arbitrary Windows C++ projects indexable.
- `scip-clang` under Linux needs Linux-readable source paths and compiler arguments. If `compile_commands.json` contains `C:\...`, `cl.exe`, Visual Studio include paths, or unmapped absolute Windows paths, fail early with a clear message telling the user to generate `compile_commands.json` inside WSL/Docker or provide a compatible image/toolchain.

## File Map

- Modify `crates/scip-io-core/src/indexer/mod.rs`
  - Add execution backend metadata to `IndexerEntry`.
  - Add native support reason fields without making logical language rows disappear.
- Modify `crates/scip-io-core/src/indexer/registry.rs`
  - Mark `scip-ruby` and `scip-clang` native Windows install support as unsupported.
  - Attach WSL/Docker backend capability metadata for those two indexers.
- Create `crates/scip-io-core/src/indexer/backend.rs`
  - Backend selection, probe, path translation, Linux binary cache preparation, and command wrapping.
- Modify `crates/scip-io-core/src/indexer/install.rs`
  - Keep current native installer behavior for non-Windows and supported Windows indexers.
  - Add Linux asset resolution/download helpers reusable by WSL/Docker without pretending the native Windows binary is installed.
- Modify `crates/scip-io-core/src/indexer/runner.rs`
  - Route protected indexer runs through `backend::prepare_execution`.
  - Preserve temp output, `kill_on_drop(true)`, stdout/stderr capture, compaction, validation, and atomic publish.
  - Add C/C++ backend preflight for Linux-compatible `compile_commands.json`.
- Modify `crates/scip-io-core/src/config/mod.rs`
  - Add optional backend configuration fields.
- Modify `crates/scip-io-cli/src/cli/mod.rs`
  - Add optional `--backend auto|native|wsl|docker|disabled` to `index`, `install`, and `status` only if needed after config support; do not make it required.
- Modify `crates/scip-io-cli/src/cli/index.rs`
  - Load backend config and pass selected backend preferences into core runner.
  - Show backend in dry-run output.
- Modify `crates/scip-io-cli/src/cli/install.rs`
  - Report native Windows unsupported for Ruby/Clang and explain WSL/Docker alternatives.
- Modify `crates/scip-io-cli/src/cli/status.rs`
  - Show native support, selected backend availability, and actionable missing-prerequisite text.
- Modify `src-tauri/src/commands.rs`
  - Surface the same status fields in GUI payloads and use the same core runner behavior.
- Modify `gui/src/components/Settings.ts`, `gui/src/components/Dashboard.ts`, and related state/types if status payload fields require UI rendering.
- Modify `README.md` and `CHANGELOG.md`
  - Document native Windows unsupported state and WSL/Docker fallback requirements.
- Add focused tests in `crates/scip-io-core/src/indexer/backend.rs` and existing runner/install/status test modules.

---

## Chunk 1: Model Native Support Separately From Backend Support

### Task 1: Add Backend Types

**Files:**
- Create: `crates/scip-io-core/src/indexer/backend.rs`
- Modify: `crates/scip-io-core/src/indexer/mod.rs`

- [ ] **Step 1: Write failing unit tests for backend capability modeling**

Add tests proving:

```rust
assert!(!entry.native_supported_on_current_platform());
assert!(entry.backend_capabilities.supports_wsl);
assert!(entry.backend_capabilities.supports_docker);
```

for `scip-ruby` and `scip-clang` on Windows, while Linux/macOS keep native support.

- [ ] **Step 2: Run the targeted tests**

Run: `cargo test -p scip-io-core windows_linux_backend_capabilities -- --nocapture`

Expected: FAIL because the backend types and helpers do not exist.

- [ ] **Step 3: Implement the types**

Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionBackendKind {
    Auto,
    Native,
    Wsl,
    Docker,
    Disabled,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BackendCapabilities {
    pub supports_wsl: bool,
    pub supports_docker: bool,
    pub native_windows_unsupported_reason: Option<String>,
}
```

Add helper methods on `IndexerEntry`:

```rust
pub fn native_supported_on_current_platform(&self) -> bool;
pub fn windows_native_unsupported_reason(&self) -> Option<&str>;
```

- [ ] **Step 4: Run tests again**

Run: `cargo test -p scip-io-core windows_linux_backend_capabilities -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/scip-io-core/src/indexer/mod.rs crates/scip-io-core/src/indexer/backend.rs
git commit -m "feat(indexers): model linux execution backends"
```

### Task 2: Mark Ruby And Clang Native Windows Unsupported

**Files:**
- Modify: `crates/scip-io-core/src/indexer/registry.rs`
- Modify tests in: `crates/scip-io-core/src/indexer/registry.rs`

- [ ] **Step 1: Write failing registry tests**

Assert that on Windows:

- `scip-ruby` reports native unsupported with reason "upstream publishes Linux/macOS assets only".
- `scip-clang` reports native unsupported with reason "upstream publishes Linux/macOS assets only".
- Both advertise WSL and Docker backend support.
- Other Windows-capable indexers are unchanged.

- [ ] **Step 2: Run registry tests**

Run: `cargo test -p scip-io-core registry -- --nocapture`

Expected: FAIL until registry metadata is updated.

- [ ] **Step 3: Update registry entries**

Keep `GitHubBinary` metadata usable for Linux asset resolution, but add backend capabilities:

```rust
backend_capabilities: BackendCapabilities {
    supports_wsl: true,
    supports_docker: true,
    native_windows_unsupported_reason: Some(
        "Native Windows binaries are not published upstream; use WSL or Docker to run the Linux binary.".into(),
    ),
}
```

- [ ] **Step 4: Run registry tests**

Run: `cargo test -p scip-io-core registry -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/scip-io-core/src/indexer/registry.rs
git commit -m "fix(indexers): mark ruby and clang native windows unsupported"
```

---

## Chunk 2: Probe And Prepare WSL/Docker Backends

### Task 3: Implement WSL Probe And Path Translation

**Files:**
- Modify: `crates/scip-io-core/src/indexer/backend.rs`

- [ ] **Step 1: Write unit tests for Windows-to-WSL path conversion**

Test pure conversion fallback for:

```text
C:\Users\alice\repo -> /mnt/c/Users/alice/repo
F:\Claude\projects\sentry -> /mnt/f/Claude/projects/sentry
\\?\F:\Claude\projects\sentry -> /mnt/f/Claude/projects/sentry
```

Also design an integration-probe function that shells out to `wsl.exe wslpath -a -u <path>` when available, with a pure fallback for tests.

- [ ] **Step 2: Run tests**

Run: `cargo test -p scip-io-core wsl_path -- --nocapture`

Expected: FAIL before implementation.

- [ ] **Step 3: Implement WSL helpers**

Add:

```rust
pub struct WslBackend {
    pub distro: Option<String>,
}

pub async fn probe_wsl() -> BackendProbeResult;
pub async fn wsl_path_for_windows_path(path: &Path) -> Result<String>;
pub fn fallback_wsl_path_for_windows_path(path: &Path) -> Result<String>;
```

Use current WSL docs behavior:

- `wsl <command>` runs Linux commands from Windows.
- Windows drives are available as `/mnt/<drive-letter>`.
- `wslpath -u` converts Windows paths to WSL paths when present.

- [ ] **Step 4: Run tests**

Run: `cargo test -p scip-io-core wsl_path -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/scip-io-core/src/indexer/backend.rs
git commit -m "feat(indexers): add wsl backend probing"
```

### Task 4: Implement Docker Probe And Mount Planning

**Files:**
- Modify: `crates/scip-io-core/src/indexer/backend.rs`

- [ ] **Step 1: Write tests for Docker mount planning**

Test that:

```text
F:\Claude\projects\sentry -> --mount type=bind,source=F:\Claude\projects\sentry,target=/workspace
```

and temp/cache directories mount to stable Linux paths:

```text
/workspace
/tmp/scip-io-output
/cache/scip-io
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p scip-io-core docker_mount -- --nocapture`

Expected: FAIL before implementation.

- [ ] **Step 3: Implement Docker helpers**

Add:

```rust
pub struct DockerBackend {
    pub image: String,
}

pub async fn probe_docker() -> BackendProbeResult;
pub fn docker_mount_plan(project_root: &Path, temp_dir: &Path) -> Result<DockerMountPlan>;
```

Default image strategy:

- `scip-ruby` / `scip-clang`: use `ubuntu:24.04` as the default Docker backend image so upstream Linux binaries have a compatible glibc. Projects that need extra compilers, headers, or package managers should set `docker_image` to a custom prepared image.
- `scip-clang`: require configurable image and warn that the image must include compilers/headers matching `compile_commands.json`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p scip-io-core docker_mount -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/scip-io-core/src/indexer/backend.rs
git commit -m "feat(indexers): add docker backend probing"
```

---

## Chunk 3: Linux Binary Acquisition Without Native Windows Install

### Task 5: Reuse GitHub Asset Resolution For Linux Assets

**Files:**
- Modify: `crates/scip-io-core/src/indexer/install.rs`
- Modify: `crates/scip-io-core/src/indexer/backend.rs`

- [ ] **Step 1: Write failing tests for Linux asset names**

Assert:

```text
scip-ruby x86_64 linux -> scip-ruby-x86_64-linux
scip-clang x86_64 linux -> scip-clang-x86_64-linux
```

even when `cfg!(windows)` is true in the caller.

- [ ] **Step 2: Run tests**

Run: `cargo test -p scip-io-core linux_asset_resolution -- --nocapture`

Expected: FAIL because asset resolution currently uses host platform helpers.

- [ ] **Step 3: Add target-platform asset helpers**

Introduce:

```rust
pub enum IndexerAssetPlatform {
    Host,
    LinuxX86_64,
    LinuxAarch64,
}

pub async fn resolve_latest_compatible_version_for_platform(
    entry: &IndexerEntry,
    platform: IndexerAssetPlatform,
) -> Result<String>;

pub async fn download_github_binary_for_platform(
    entry: &IndexerEntry,
    version: &str,
    platform: IndexerAssetPlatform,
    dest_dir: &Path,
    progress: &dyn ProgressHandler,
) -> Result<PathBuf>;
```

Do not change native install behavior for other indexers.

- [ ] **Step 4: Run tests**

Run: `cargo test -p scip-io-core linux_asset_resolution -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/scip-io-core/src/indexer/install.rs crates/scip-io-core/src/indexer/backend.rs
git commit -m "feat(indexers): resolve linux assets for remote backends"
```

### Task 6: Prepare Linux Binary In WSL And Docker Cache

**Files:**
- Modify: `crates/scip-io-core/src/indexer/backend.rs`

- [ ] **Step 1: Write fake-backend tests**

Use command-builder tests, not real WSL/Docker, to assert:

- WSL command copies or references the Linux binary at a WSL path and runs `chmod +x`.
- Docker command mounts a cache directory or volume and runs the Linux binary at a Linux path.
- Neither path marks the native Windows install as managed installed.

- [ ] **Step 2: Run tests**

Run: `cargo test -p scip-io-core backend_command_builder -- --nocapture`

Expected: FAIL before implementation.

- [ ] **Step 3: Implement command builders**

Add:

```rust
pub struct PreparedBackendCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub current_dir: Option<PathBuf>,
    pub display_command: String,
    pub output_path_on_host: PathBuf,
}

pub async fn prepare_execution(
    request: BackendExecutionRequest<'_>,
) -> Result<PreparedBackendCommand>;
```

For WSL, generate:

```powershell
wsl.exe --cd <wsl-project-root> -- sh -lc '<binary> <args>'
```

For Docker, generate:

```powershell
docker run --rm --mount type=bind,source=<project>,target=/workspace --workdir /workspace <image> sh -lc '<binary> <args>'
```

Quote arguments with a dedicated helper; do not hand-concatenate user paths without quoting.

- [ ] **Step 4: Run tests**

Run: `cargo test -p scip-io-core backend_command_builder -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/scip-io-core/src/indexer/backend.rs
git commit -m "feat(indexers): prepare linux backend commands"
```

---

## Chunk 4: Route Runner Through Backends

### Task 7: Add Backend Preference To Config And Runner API

**Files:**
- Modify: `crates/scip-io-core/src/config/mod.rs`
- Modify: `crates/scip-io-core/src/indexer/runner.rs`
- Modify: `crates/scip-io-cli/src/cli/index.rs`
- Modify: `src-tauri/src/commands.rs`

- [ ] **Step 1: Write config parsing tests**

Add `.scip-io.toml` coverage:

```toml
[settings]
linux_indexer_backend = "auto"

[indexer.ruby]
backend = "wsl"

[indexer.cpp]
backend = "docker"
docker_image = "my/scip-clang-runtime:latest"
wsl_distro = "Ubuntu-24.04"
```

- [ ] **Step 2: Run config tests**

Run: `cargo test -p scip-io-core config_backend -- --nocapture`

Expected: FAIL before fields exist.

- [ ] **Step 3: Implement config fields**

Add optional fields:

```rust
pub linux_indexer_backend: Option<ExecutionBackendKind>,
pub backend: Option<ExecutionBackendKind>,
pub docker_image: Option<String>,
pub wsl_distro: Option<String>,
```

Keep defaults as `Auto`.

- [ ] **Step 4: Thread preferences into runner calls**

Add a non-breaking wrapper:

```rust
pub async fn run_indexer_with_configs_and_backend(
    binary: Option<&Path>,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    config_paths: &[PathBuf],
    backend: BackendPreference,
) -> Result<PathBuf>;
```

Keep existing `run_indexer_with_configs(...)` as a native/auto wrapper for current call sites until CLI/GUI are updated.

- [ ] **Step 5: Run tests**

Run: `cargo test -p scip-io-core config_backend runner_backend -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/scip-io-core/src/config/mod.rs crates/scip-io-core/src/indexer/runner.rs crates/scip-io-cli/src/cli/index.rs src-tauri/src/commands.rs
git commit -m "feat(indexers): pass backend preferences into runner"
```

### Task 8: Preserve Protected Output Contract Across Backends

**Files:**
- Modify: `crates/scip-io-core/src/indexer/runner.rs`
- Modify tests in: `crates/scip-io-core/src/indexer/runner.rs`

- [ ] **Step 1: Write fake backend integration tests**

Test that a fake WSL/Docker command:

- Writes temp output only.
- Gets normalized, compacted, validated.
- Publishes final `<language>.scip` only after success.
- Leaves existing final output untouched on failure.
- Captures stderr/stdout in the error.

- [ ] **Step 2: Run tests**

Run: `cargo test -p scip-io-core backend_runner -- --nocapture`

Expected: FAIL until runner uses prepared backend commands.

- [ ] **Step 3: Implement runner routing**

Inside `run_indexer_to_temp_output_with_args`, replace direct `Command::new(request.binary)` with a prepared execution command when backend is not native. Preserve:

```rust
cmd.current_dir(...);
cmd.kill_on_drop(true);
cmd.output().await;
```

Native path should remain byte-for-byte equivalent for existing tests.

- [ ] **Step 4: Run full runner tests**

Run: `cargo test -p scip-io-core runner -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/scip-io-core/src/indexer/runner.rs crates/scip-io-core/src/indexer/backend.rs
git commit -m "feat(indexers): run protected indexers through linux backends"
```

---

## Chunk 5: C/C++ Backend Safety Preflight

### Task 9: Validate Linux-Compatible Compile Commands

**Files:**
- Modify: `crates/scip-io-core/src/indexer/backend.rs`
- Modify: `crates/scip-io-core/src/indexer/planner.rs` if compile-command parsing helpers already belong there

- [ ] **Step 1: Write tests for incompatible compile databases**

Inputs that should fail before invoking `scip-clang` through WSL/Docker:

```json
[{ "directory": "F:\\Claude\\projects\\foo", "command": "cl.exe /I C:\\SDK foo.cpp", "file": "F:\\Claude\\projects\\foo\\foo.cpp" }]
```

Inputs that should pass or be staged/re-written:

```json
[{ "directory": "/workspace", "command": "clang++ -I include -c src/foo.cpp", "file": "src/foo.cpp" }]
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p scip-io-core compile_commands_backend_preflight -- --nocapture`

Expected: FAIL before implementation.

- [ ] **Step 3: Implement preflight**

Before WSL/Docker `scip-clang`:

- Load `compile_commands.json` with serde.
- Reject `cl.exe`, `clang-cl.exe`, Visual Studio toolchain paths, and absolute Windows drive paths unless a future transform explicitly handles them.
- For Docker, stage a backend-local `compile_commands.scip-io.json` with `/workspace` paths when entries are repo-relative or under the project root.
- For WSL, prefer WSL path conversion for entries under the project root.

- [ ] **Step 4: Run tests**

Run: `cargo test -p scip-io-core compile_commands_backend_preflight -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/scip-io-core/src/indexer/backend.rs crates/scip-io-core/src/indexer/planner.rs
git commit -m "fix(indexers): preflight clang linux backend inputs"
```

---

## Chunk 6: CLI And GUI Status

### Task 10: Update CLI Status, Install, And Dry-Run Messaging

**Files:**
- Modify: `crates/scip-io-cli/src/cli/status.rs`
- Modify: `crates/scip-io-cli/src/cli/install.rs`
- Modify: `crates/scip-io-cli/src/cli/index.rs`
- Modify tests in those modules

- [ ] **Step 1: Write output tests**

Expected `status --format json` fields:

```json
{
  "indexer": "scip-clang",
  "native_supported": false,
  "native_unsupported_reason": "Native Windows binaries are not published upstream...",
  "backend_support": ["wsl", "docker"],
  "selected_backend": "auto",
  "backend_available": true
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p scip-io-cli status_backend install_backend dry_run_backend -- --nocapture`

Expected: FAIL before CLI output changes.

- [ ] **Step 3: Implement CLI messages**

Rules:

- `scip-io install ruby` on Windows should not try to download a nonexistent native asset.
- It should say native install is unsupported and `scip-io index --lang ruby` can use WSL/Docker if configured/available.
- `dry-run` should display backend command shape without exposing huge shell scripts.

- [ ] **Step 4: Run tests**

Run: `cargo test -p scip-io-cli status_backend install_backend dry_run_backend -- --nocapture`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/scip-io-cli/src/cli/status.rs crates/scip-io-cli/src/cli/install.rs crates/scip-io-cli/src/cli/index.rs
git commit -m "feat(cli): surface windows linux indexer backends"
```

### Task 11: Update GUI Status Payloads

**Files:**
- Modify: `src-tauri/src/commands.rs`
- Modify: `gui/src/state/store.ts`
- Modify: `gui/src/components/Settings.ts`
- Modify: `gui/src/components/Dashboard.ts`

- [ ] **Step 1: Write Rust payload tests**

Assert `IndexerStatusInfo` includes native support and backend details for Ruby/Clang.

- [ ] **Step 2: Run Tauri command tests**

Run: `cargo test -p scip-io-gui indexer_status -- --nocapture`

Expected: FAIL before payload fields exist.

- [ ] **Step 3: Implement payload and UI text**

GUI should show:

- "Native Windows binary unavailable"
- "Can run via WSL" or "Can run via Docker"
- Missing prerequisites like "WSL not detected" or "Docker not running"

Do not offer a native install button for these two indexers on Windows.

- [ ] **Step 4: Run tests/build**

Run:

```bash
cargo test -p scip-io-gui indexer_status -- --nocapture
npm run build
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands.rs gui/src/state/store.ts gui/src/components/Settings.ts gui/src/components/Dashboard.ts
git commit -m "feat(gui): show linux backend support for windows indexers"
```

---

## Chunk 7: Real Backend Smoke Tests And Documentation

### Task 12: Add Real Smoke Fixtures

**Files:**
- Create: `crates/scip-io-core/tests/windows_linux_backend_smoke.rs` if cross-crate integration tests fit better than module tests
- Modify: `.github/workflows/ci.yml` only if adding non-required optional smoke jobs

- [ ] **Step 1: Add ignored real-environment tests**

Tests should be `#[ignore]` and only run when prerequisites exist:

- WSL available with Linux x86_64.
- Docker available and daemon running.
- Ruby fixture with a tiny `*.rb`.
- C++ fixture with Linux-compatible `compile_commands.json`.

- [ ] **Step 2: Run ignored tests locally when available**

Run:

```bash
cargo test -p scip-io-core --test windows_linux_backend_smoke -- --ignored --nocapture
```

Expected: PASS when WSL/Docker prerequisites are installed; SKIP with clear reason otherwise.

- [ ] **Step 3: Commit**

```bash
git add crates/scip-io-core/tests/windows_linux_backend_smoke.rs
git commit -m "test(indexers): add optional linux backend smoke tests"
```

### Task 13: Update Docs

**Files:**
- Modify: `README.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Document support matrix**

Update Ruby and C/C++ rows:

- Native Windows: unsupported upstream.
- Windows fallback: WSL/Docker Linux backend.
- C/C++ requirement: Linux-compatible `compile_commands.json` and matching backend toolchain.

- [ ] **Step 2: Add examples**

Include:

```toml
[indexer.ruby]
backend = "wsl"

[indexer.cpp]
backend = "docker"
docker_image = "my/scip-clang-runtime:latest"
```

and CLI examples:

```powershell
scip-io status --verbose
scip-io index --path F:\Claude\projects\rails --lang ruby
scip-io index --path F:\Claude\projects\llvm-project --lang cpp
```

- [ ] **Step 3: Commit docs**

```bash
git add README.md CHANGELOG.md
git commit -m "docs: document windows linux indexer backends"
```

---

## Chunk 8: Final Verification

### Task 14: Run Full Verification

**Files:**
- No new files unless verification reveals issues.

- [ ] **Step 1: Rust formatting**

Run: `cargo fmt --all --check`

Expected: PASS.

- [ ] **Step 2: Rust tests**

Run: `cargo test --workspace`

Expected: PASS.

- [ ] **Step 3: Clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`

Expected: PASS.

- [ ] **Step 4: GUI build**

Run: `npm run build`

Expected: PASS.

- [ ] **Step 5: Release builds**

Run:

```bash
cargo build --workspace --release
cargo tauri build
```

Expected: PASS. If `scip-io-gui.exe` is locked, stop the stale GUI process and retry.

- [ ] **Step 6: Real manual acceptance**

On Windows with WSL:

```powershell
target\release\scip-io.exe status --verbose
target\release\scip-io.exe index --path F:\Claude\projects\scip-benchmark-repos\rails --lang ruby --no-merge --timeout 1200
target\release\scip-io.exe validate F:\Claude\projects\scip-benchmark-repos\rails\ruby.scip
```

Expected: `ruby.scip` is valid and contains Ruby documents.

On Windows with Docker and a Linux-compatible C++ fixture:

```powershell
target\release\scip-io.exe index --path F:\Claude\projects\scip-benchmark-repos\cpp-smoke --lang cpp --no-merge --timeout 1200
target\release\scip-io.exe validate F:\Claude\projects\scip-benchmark-repos\cpp-smoke\cpp.scip
```

Expected: `cpp.scip` is valid and contains C/C++ documents.

- [ ] **Step 7: Final commit**

```bash
git status --short
git diff --check
```

Expected: no unexpected files and no whitespace errors beyond normal Windows CRLF warnings.

Commit any verification fixes with a focused message.

---

## Risks And Guardrails

- Do not make WSL/Docker output bypass `postprocess_scip_output`; all `.scip` files must still normalize, compact, validate, and publish atomically.
- Do not report `scip-ruby` or `scip-clang` as natively installed on Windows because a Linux backend exists.
- Do not silently rewrite incompatible Visual Studio compile commands into Linux commands. Fail with a concrete explanation instead.
- Do not require Docker for users who have WSL, or WSL for users who have Docker.
- Do not add a large hidden runtime image download without showing progress and documenting the cache location.
- Keep backend feature scope limited to `scip-ruby` and `scip-clang` until another upstream indexer has the same platform gap.
