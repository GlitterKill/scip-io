use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::indexer::IndexerEntry;
use crate::indexer::install::{
    IndexerAssetPlatform, download_github_binary_for_platform, expected_github_assets_for_platform,
};
use crate::indexer::install_dir;
use crate::process::hidden_tokio_command;
use crate::progress::NoopHandler;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionBackendKind {
    #[default]
    Auto,
    Native,
    Wsl,
    Docker,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BackendCapabilities {
    #[serde(default)]
    pub supports_wsl: bool,
    #[serde(default)]
    pub supports_docker: bool,
    #[serde(default)]
    pub native_windows_unsupported_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackendPreference {
    pub kind: ExecutionBackendKind,
    pub docker_image: Option<String>,
    pub wsl_distro: Option<String>,
}

impl BackendPreference {
    pub fn auto() -> Self {
        Self::default()
    }

    pub fn native() -> Self {
        Self {
            kind: ExecutionBackendKind::Native,
            docker_image: None,
            wsl_distro: None,
        }
    }

    pub fn disabled() -> Self {
        Self {
            kind: ExecutionBackendKind::Disabled,
            docker_image: None,
            wsl_distro: None,
        }
    }
}

impl BackendCapabilities {
    pub fn native() -> Self {
        Self::default()
    }

    pub fn windows_linux_backends(reason: impl Into<String>) -> Self {
        Self {
            supports_wsl: true,
            supports_docker: true,
            native_windows_unsupported_reason: Some(reason.into()),
        }
    }

    pub fn wsl_optional() -> Self {
        Self {
            supports_wsl: true,
            supports_docker: false,
            native_windows_unsupported_reason: None,
        }
    }

    pub fn backend_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.supports_wsl {
            names.push("wsl");
        }
        if self.supports_docker {
            names.push("docker");
        }
        names
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendProbeResult {
    pub kind: ExecutionBackendKind,
    pub available: bool,
    pub detail: Option<String>,
}

impl BackendProbeResult {
    pub fn available(kind: ExecutionBackendKind) -> Self {
        Self {
            kind,
            available: true,
            detail: None,
        }
    }

    pub fn unavailable(kind: ExecutionBackendKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            available: false,
            detail: Some(detail.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WslBackend {
    pub distro: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerBackend {
    pub image: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerMountPlan {
    pub project_source: PathBuf,
    pub project_target: String,
    pub temp_source: PathBuf,
    pub temp_target: String,
    pub cache_source: PathBuf,
    pub cache_target: String,
}

pub const DEFAULT_DOCKER_IMAGE: &str = "ubuntu:24.04";
pub const DOCKER_WORKSPACE: &str = "/workspace";
pub const DOCKER_TEMP: &str = "/tmp/scip-io-output";
pub const DOCKER_CACHE: &str = "/cache/scip-io";

#[derive(Debug, Clone)]
pub struct BackendExecutionRequest<'a> {
    pub native_binary: Option<&'a Path>,
    pub entry: &'a IndexerEntry,
    pub project_root: &'a Path,
    pub temp_dir: &'a Path,
    pub output_name: &'a str,
    pub args: Vec<OsString>,
    pub preference: BackendPreference,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedBackendCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub current_dir: Option<PathBuf>,
    pub display_command: String,
    pub output_path_on_host: PathBuf,
    pub output_path_for_process: OsString,
    pub backend: ExecutionBackendKind,
}

pub async fn probe_wsl() -> BackendProbeResult {
    probe_wsl_with_distro(None).await
}

pub async fn probe_wsl_with_distro(distro: Option<&str>) -> BackendProbeResult {
    let mut command = hidden_tokio_command("wsl.exe");
    command.args(wsl_probe_args(distro));

    match command.output().await {
        Ok(output) if output.status.success() => {
            BackendProbeResult::available(ExecutionBackendKind::Wsl)
        }
        Ok(output) => BackendProbeResult::unavailable(
            ExecutionBackendKind::Wsl,
            command_failure_detail(&wsl_display_command(distro, "true"), &output),
        ),
        Err(error) => BackendProbeResult::unavailable(ExecutionBackendKind::Wsl, error.to_string()),
    }
}

pub async fn probe_docker() -> BackendProbeResult {
    let mut command = hidden_tokio_command("docker");
    match command
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            BackendProbeResult::available(ExecutionBackendKind::Docker)
        }
        Ok(output) => BackendProbeResult::unavailable(
            ExecutionBackendKind::Docker,
            format!("docker info exited with {}", output.status),
        ),
        Err(error) => {
            BackendProbeResult::unavailable(ExecutionBackendKind::Docker, error.to_string())
        }
    }
}

pub async fn backend_availability_for_entry(entry: &IndexerEntry) -> Vec<BackendProbeResult> {
    backend_availability_for_entry_with_preference(entry, &BackendPreference::auto()).await
}

pub async fn backend_availability_for_entry_with_preference(
    entry: &IndexerEntry,
    preference: &BackendPreference,
) -> Vec<BackendProbeResult> {
    let mut probes = Vec::new();
    if entry.backend_capabilities.supports_wsl {
        probes.push(probe_wsl_with_distro(preference.wsl_distro.as_deref()).await);
    }
    if entry.backend_capabilities.supports_docker {
        probes.push(probe_docker().await);
    }
    probes
}

pub async fn prepare_execution(
    request: BackendExecutionRequest<'_>,
) -> Result<PreparedBackendCommand> {
    let backend = select_backend(&request).await?;
    match backend {
        ExecutionBackendKind::Native => prepare_native_command(&request),
        ExecutionBackendKind::Wsl => {
            preflight_linux_backend_inputs(&request, ExecutionBackendKind::Wsl)?;
            let binary = prepare_linux_backend_binary(request.entry).await?;
            chmod_wsl_binary(&binary, request.preference.wsl_distro.as_deref()).await?;
            prepare_wsl_command(&request, &binary).await
        }
        ExecutionBackendKind::Docker => {
            preflight_linux_backend_inputs(&request, ExecutionBackendKind::Docker)?;
            let binary = prepare_linux_backend_binary(request.entry).await?;
            prepare_docker_command(&request, &binary)
        }
        ExecutionBackendKind::Auto | ExecutionBackendKind::Disabled => {
            bail!("internal error: unresolved backend {backend:?}")
        }
    }
}

async fn select_backend(request: &BackendExecutionRequest<'_>) -> Result<ExecutionBackendKind> {
    match request.preference.kind {
        ExecutionBackendKind::Disabled => {
            bail!(
                "{} indexing is disabled by backend configuration",
                request.entry.indexer_name
            )
        }
        ExecutionBackendKind::Native => {
            if request.native_binary.is_none() {
                bail_native_unavailable(request.entry)?;
            }
            Ok(ExecutionBackendKind::Native)
        }
        ExecutionBackendKind::Wsl => {
            ensure_backend_supported(request.entry, ExecutionBackendKind::Wsl)?;
            let probe = probe_wsl_with_distro(request.preference.wsl_distro.as_deref()).await;
            if !probe.available {
                bail!(
                    "{} requires WSL backend, but WSL is unavailable: {}",
                    request.entry.indexer_name,
                    probe.detail.unwrap_or_else(|| "probe failed".into())
                );
            }
            Ok(ExecutionBackendKind::Wsl)
        }
        ExecutionBackendKind::Docker => {
            ensure_backend_supported(request.entry, ExecutionBackendKind::Docker)?;
            let probe = probe_docker().await;
            if !probe.available {
                bail!(
                    "{} requires Docker backend, but Docker is unavailable: {}",
                    request.entry.indexer_name,
                    probe.detail.unwrap_or_else(|| "probe failed".into())
                );
            }
            Ok(ExecutionBackendKind::Docker)
        }
        ExecutionBackendKind::Auto => {
            if request.entry.native_supported_on_current_platform()
                && request.native_binary.is_some()
            {
                return Ok(ExecutionBackendKind::Native);
            }

            let mut failures = Vec::new();
            if request.entry.backend_capabilities.supports_wsl {
                let probe = probe_wsl_with_distro(request.preference.wsl_distro.as_deref()).await;
                if probe.available {
                    return Ok(ExecutionBackendKind::Wsl);
                }
                failures.push(format!(
                    "WSL unavailable: {}",
                    probe.detail.unwrap_or_else(|| "probe failed".into())
                ));
            }
            if request.entry.backend_capabilities.supports_docker {
                let probe = probe_docker().await;
                if probe.available {
                    return Ok(ExecutionBackendKind::Docker);
                }
                failures.push(format!(
                    "Docker unavailable: {}",
                    probe.detail.unwrap_or_else(|| "probe failed".into())
                ));
            }

            if !request.entry.native_supported_on_current_platform() {
                let reason = request
                    .entry
                    .windows_native_unsupported_reason()
                    .unwrap_or("native backend is unavailable on this platform");
                bail!(
                    "{} cannot run natively on this platform: {}. {}",
                    request.entry.indexer_name,
                    reason,
                    failures.join("; ")
                );
            }
            bail_native_unavailable(request.entry)
        }
    }
}

fn bail_native_unavailable(entry: &IndexerEntry) -> Result<ExecutionBackendKind> {
    if let Some(reason) = entry.windows_native_unsupported_reason() {
        bail!(
            "{} cannot run natively on Windows: {}",
            entry.indexer_name,
            reason
        );
    }
    bail!("{} native binary is not installed", entry.indexer_name)
}

fn ensure_backend_supported(entry: &IndexerEntry, backend: ExecutionBackendKind) -> Result<()> {
    let supported = match backend {
        ExecutionBackendKind::Wsl => entry.backend_capabilities.supports_wsl,
        ExecutionBackendKind::Docker => entry.backend_capabilities.supports_docker,
        _ => true,
    };
    if !supported {
        bail!(
            "{} does not support the {backend:?} backend",
            entry.indexer_name
        );
    }
    Ok(())
}

fn prepare_native_command(request: &BackendExecutionRequest<'_>) -> Result<PreparedBackendCommand> {
    let binary = request.native_binary.with_context(|| {
        format!(
            "{} native binary is required for native execution",
            request.entry.indexer_name
        )
    })?;
    let output_path_on_host = request.temp_dir.join(request.output_name);
    Ok(PreparedBackendCommand {
        program: binary.to_path_buf(),
        args: request.args.clone(),
        current_dir: Some(request.project_root.to_path_buf()),
        display_command: display_command(binary, &request.args),
        output_path_for_process: output_path_on_host.as_os_str().to_os_string(),
        output_path_on_host,
        backend: ExecutionBackendKind::Native,
    })
}

async fn prepare_wsl_command(
    request: &BackendExecutionRequest<'_>,
    binary_on_host: &Path,
) -> Result<PreparedBackendCommand> {
    let distro = request.preference.wsl_distro.as_deref();
    let project_root = wsl_path_for_windows_path_with_distro(request.project_root, distro).await?;
    let temp_dir = wsl_path_for_windows_path_with_distro(request.temp_dir, distro).await?;
    let binary = wsl_path_for_windows_path_with_distro(binary_on_host, distro).await?;
    let output_path_on_host = request.temp_dir.join(request.output_name);
    let output_path_for_process = linux_join(&temp_dir, request.output_name);
    let mapped_args = map_backend_args(
        &request.args,
        &[
            (request.temp_dir.to_path_buf(), temp_dir.clone()),
            (request.project_root.to_path_buf(), project_root.clone()),
            (output_path_on_host.clone(), output_path_for_process.clone()),
        ],
    );

    let mut args = Vec::new();
    push_wsl_distro_os_args(&mut args, distro);
    args.push(OsString::from("--cd"));
    args.push(OsString::from(&project_root));
    args.push(OsString::from("--"));
    args.push(OsString::from(&binary));
    args.extend(mapped_args);

    Ok(PreparedBackendCommand {
        program: PathBuf::from("wsl.exe"),
        display_command: format!(
            "{} --cd {project_root} -- {}",
            wsl_display_prefix(distro),
            binary
        ),
        args,
        current_dir: None,
        output_path_for_process: OsString::from(output_path_for_process),
        output_path_on_host,
        backend: ExecutionBackendKind::Wsl,
    })
}

fn prepare_docker_command(
    request: &BackendExecutionRequest<'_>,
    binary_on_host: &Path,
) -> Result<PreparedBackendCommand> {
    let output_path_on_host = request.temp_dir.join(request.output_name);
    let output_path_for_process = linux_join(DOCKER_TEMP, request.output_name);
    let mount = docker_mount_plan(request.project_root, request.temp_dir)?;
    let relative_binary = binary_on_host
        .strip_prefix(&mount.cache_source)
        .with_context(|| {
            format!(
                "backend binary {} is outside Docker cache mount {}",
                binary_on_host.display(),
                mount.cache_source.display()
            )
        })?;
    let binary = linux_join_path(&mount.cache_target, relative_binary);
    let mapped_args = map_backend_args(
        &request.args,
        &[
            (request.temp_dir.to_path_buf(), DOCKER_TEMP.to_owned()),
            (
                request.project_root.to_path_buf(),
                DOCKER_WORKSPACE.to_owned(),
            ),
            (output_path_on_host.clone(), output_path_for_process.clone()),
        ],
    );

    let image = request
        .preference
        .docker_image
        .clone()
        .unwrap_or_else(|| DEFAULT_DOCKER_IMAGE.to_owned());
    let mut args = vec![
        OsString::from("run"),
        OsString::from("--rm"),
        OsString::from("--mount"),
        OsString::from(format!(
            "type=bind,source={},target={}",
            mount.project_source.display(),
            mount.project_target
        )),
        OsString::from("--mount"),
        OsString::from(format!(
            "type=bind,source={},target={}",
            mount.temp_source.display(),
            mount.temp_target
        )),
        OsString::from("--mount"),
        OsString::from(format!(
            "type=bind,source={},target={}",
            mount.cache_source.display(),
            mount.cache_target
        )),
        OsString::from("--workdir"),
        OsString::from(DOCKER_WORKSPACE),
        OsString::from(image.clone()),
        OsString::from("sh"),
        OsString::from("-c"),
        OsString::from(r#"chmod +x "$1" && exec "$@""#),
        OsString::from("sh"),
        OsString::from(&binary),
    ];
    args.extend(mapped_args);

    Ok(PreparedBackendCommand {
        program: PathBuf::from("docker"),
        display_command: format!("docker run --rm --workdir {DOCKER_WORKSPACE} {image} {binary}"),
        args,
        current_dir: None,
        output_path_for_process: OsString::from(output_path_for_process),
        output_path_on_host,
        backend: ExecutionBackendKind::Docker,
    })
}

async fn prepare_linux_backend_binary(entry: &IndexerEntry) -> Result<PathBuf> {
    let platform = linux_asset_platform_for_host();
    let version = entry.version.clone();
    let binary_dir = linux_backend_binary_dir(entry, &version, platform);
    let binary = binary_dir.join(&entry.binary_name);
    if binary.exists() {
        return Ok(binary);
    }

    // Validate the asset name early so unsupported release layouts fail before
    // an opaque Docker/WSL invocation.
    let _ = expected_github_assets_for_platform(entry, &version, platform)?;
    download_github_binary_for_platform(entry, &version, platform, &binary_dir, &NoopHandler).await
}

fn linux_backend_binary_dir(
    entry: &IndexerEntry,
    version: &str,
    platform: IndexerAssetPlatform,
) -> PathBuf {
    install_dir()
        .join("linux-backends")
        .join(&entry.indexer_name)
        .join(version)
        .join(platform.to_string())
}

fn linux_asset_platform_for_host() -> IndexerAssetPlatform {
    if cfg!(target_arch = "aarch64") {
        IndexerAssetPlatform::LinuxAarch64
    } else {
        IndexerAssetPlatform::LinuxX86_64
    }
}

async fn chmod_wsl_binary(binary_on_host: &Path, distro: Option<&str>) -> Result<()> {
    let binary = wsl_path_for_windows_path_with_distro(binary_on_host, distro).await?;
    let mut command = hidden_tokio_command("wsl.exe");
    push_wsl_distro_command_args(&mut command, distro);
    let output = command
        .args(["--", "chmod", "+x", &binary])
        .output()
        .await?;
    if !output.status.success() {
        bail!(
            "failed to mark Linux backend binary executable in WSL: {}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub async fn wsl_path_for_windows_path(path: &Path) -> Result<String> {
    wsl_path_for_windows_path_with_distro(path, None).await
}

pub async fn wsl_path_for_windows_path_with_distro(
    path: &Path,
    distro: Option<&str>,
) -> Result<String> {
    let wsl_input = remove_windows_extended_path_prefix(path);
    let mut command = hidden_tokio_command("wsl.exe");
    push_wsl_distro_command_args(&mut command, distro);
    match command
        .args(["--", "wslpath", "-a", "-u"])
        .arg(&wsl_input)
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let converted = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if !converted.is_empty() {
                return Ok(converted);
            }
        }
        _ => {}
    }

    fallback_wsl_path_for_windows_path(&wsl_input)
}

fn remove_windows_extended_path_prefix(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(stripped) = raw.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{stripped}"));
    }
    if let Some(stripped) = raw.strip_prefix(r"\\?\") {
        return PathBuf::from(stripped);
    }
    path.to_path_buf()
}

fn push_wsl_distro_command_args(command: &mut tokio::process::Command, distro: Option<&str>) {
    if let Some(distro) = distro {
        command.args(["-d", distro]);
    }
}

fn push_wsl_distro_os_args(args: &mut Vec<OsString>, distro: Option<&str>) {
    if let Some(distro) = distro {
        args.push(OsString::from("-d"));
        args.push(OsString::from(distro));
    }
}

fn wsl_probe_args(distro: Option<&str>) -> Vec<OsString> {
    let mut args = Vec::new();
    push_wsl_distro_os_args(&mut args, distro);
    args.push(OsString::from("--"));
    args.push(OsString::from("true"));
    args
}

fn wsl_display_prefix(distro: Option<&str>) -> String {
    match distro {
        Some(distro) => format!("wsl.exe -d {distro}"),
        None => "wsl.exe".to_string(),
    }
}

fn wsl_display_command(distro: Option<&str>, command: &str) -> String {
    format!("{} -- {}", wsl_display_prefix(distro), command)
}

fn command_failure_detail(command: &str, output: &std::process::Output) -> String {
    let stderr = process_output_text(&output.stderr);
    let stderr = stderr.trim();
    let stdout = process_output_text(&output.stdout);
    let stdout = stdout.trim();
    let detail = if !stderr.is_empty() {
        Some(stderr)
    } else if !stdout.is_empty() {
        Some(stdout)
    } else {
        None
    };
    if let Some(detail) = detail {
        format!("{command} exited with {}: {detail}", output.status)
    } else {
        format!("{command} exited with {}", output.status)
    }
}

fn process_output_text(bytes: &[u8]) -> String {
    let nul_count = bytes.iter().filter(|byte| **byte == 0).count();
    if bytes.len() >= 2 && nul_count > bytes.len() / 4 {
        let utf16 = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&utf16)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

pub fn fallback_wsl_path_for_windows_path(path: &Path) -> Result<String> {
    let raw = path.to_string_lossy().replace('\\', "/");
    let trimmed = raw.strip_prefix("//?/").unwrap_or(&raw);
    let bytes = trimmed.as_bytes();
    if bytes.len() < 3 || bytes[1] != b':' || bytes[2] != b'/' {
        bail!(
            "cannot convert non-drive Windows path to WSL path: {}",
            path.display()
        );
    }

    let drive = (bytes[0] as char).to_ascii_lowercase();
    let rest = &trimmed[3..];
    Ok(format!("/mnt/{drive}/{rest}"))
}

pub fn docker_mount_plan(project_root: &Path, temp_dir: &Path) -> Result<DockerMountPlan> {
    let project_source = remove_windows_extended_path_prefix(project_root);
    let temp_source = remove_windows_extended_path_prefix(temp_dir);
    let cache_source =
        remove_windows_extended_path_prefix(&crate::indexer::install_dir().join("linux-backends"));
    std::fs::create_dir_all(&cache_source)
        .with_context(|| format!("Cannot create {}", cache_source.display()))?;
    Ok(DockerMountPlan {
        project_source,
        project_target: DOCKER_WORKSPACE.to_owned(),
        temp_source,
        temp_target: DOCKER_TEMP.to_owned(),
        cache_source,
        cache_target: DOCKER_CACHE.to_owned(),
    })
}

pub fn preflight_compile_commands_for_linux_backend(compile_commands: &Path) -> Result<()> {
    let raw = std::fs::read_to_string(compile_commands)
        .with_context(|| format!("Failed to read {}", compile_commands.display()))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("Failed to parse {}", compile_commands.display()))?;
    let serde_json::Value::Array(commands) = value else {
        bail!("{} is not a JSON array", compile_commands.display());
    };

    for (index, command) in commands.iter().enumerate() {
        for field in ["directory", "command", "file"] {
            if let Some(value) = command.get(field).and_then(|value| value.as_str()) {
                reject_windows_compile_command_text(compile_commands, index, field, value)?;
            }
        }
        if let Some(args) = command.get("arguments").and_then(|value| value.as_array()) {
            for arg in args.iter().filter_map(|value| value.as_str()) {
                reject_windows_compile_command_text(compile_commands, index, "arguments", arg)?;
            }
        }
    }
    Ok(())
}

fn preflight_linux_backend_inputs(
    request: &BackendExecutionRequest<'_>,
    backend: ExecutionBackendKind,
) -> Result<()> {
    if request.entry.indexer_name != "scip-clang" {
        return Ok(());
    }

    let compile_commands = compile_command_paths_from_args(&request.args, request.project_root);
    if compile_commands.is_empty() {
        bail!(
            "scip-clang {backend:?} backend requires --compdb-path pointing to a Linux-compatible compile_commands.json"
        );
    }
    for path in compile_commands {
        preflight_compile_commands_for_linux_backend(&path)?;
    }
    Ok(())
}

fn compile_command_paths_from_args(args: &[OsString], project_root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for arg in args {
        let text = arg.to_string_lossy();
        if let Some(path) = text.strip_prefix("--compdb-path=") {
            let path = PathBuf::from(path);
            paths.push(if path.is_absolute() {
                path
            } else {
                project_root.join(path)
            });
        }
    }
    paths
}

fn reject_windows_compile_command_text(
    compile_commands: &Path,
    index: usize,
    field: &str,
    value: &str,
) -> Result<()> {
    let lower = value.to_ascii_lowercase().replace('\\', "/");
    if contains_windows_drive_path(value)
        || lower.contains("cl.exe")
        || lower.contains("clang-cl.exe")
        || lower.contains("microsoft visual studio")
    {
        bail!(
            "{} entry {} field '{}' is not Linux-compatible for scip-clang backends: {}. Generate compile_commands.json inside WSL/Docker or use a backend image/toolchain with Linux paths.",
            compile_commands.display(),
            index,
            field,
            value
        );
    }
    Ok(())
}

fn contains_windows_drive_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.windows(3).any(|window| {
        window[0].is_ascii_alphabetic() && window[1] == b':' && matches!(window[2], b'\\' | b'/')
    })
}

fn map_backend_args(args: &[OsString], mappings: &[(PathBuf, String)]) -> Vec<OsString> {
    args.iter()
        .map(|arg| OsString::from(map_backend_arg(arg, mappings)))
        .collect()
}

fn map_backend_arg(arg: &OsStr, mappings: &[(PathBuf, String)]) -> String {
    let mut text = arg.to_string_lossy().to_string();
    for (host, target) in mappings {
        text = replace_host_path_prefix(&text, host, target);
    }
    text
}

fn replace_host_path_prefix(text: &str, host: &Path, target: &str) -> String {
    let host_text = host.to_string_lossy();
    let variants = [host_text.to_string(), host_text.replace('\\', "/")];
    let mut result = text.to_string();
    for variant in variants {
        if variant.is_empty() {
            continue;
        }
        if let Some(pos) = result.find(&variant) {
            let before = &result[..pos];
            let after = result[pos + variant.len()..].replace('\\', "/");
            result = format!("{before}{target}{after}");
        }
    }
    result
}

fn linux_join(prefix: &str, child: &str) -> String {
    format!(
        "{}/{}",
        prefix.trim_end_matches('/'),
        child.replace('\\', "/")
    )
}

fn linux_join_path(prefix: &str, path: &Path) -> String {
    let relative = path.to_string_lossy().replace('\\', "/");
    linux_join(prefix, &relative)
}

fn display_command(program: &Path, args: &[OsString]) -> String {
    let mut parts = vec![program.display().to_string()];
    parts.extend(args.iter().map(|arg| arg.to_string_lossy().to_string()));
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::InstallMethod;
    use tempfile::TempDir;

    fn entry(indexer_name: &str) -> IndexerEntry {
        IndexerEntry {
            indexer_name: indexer_name.into(),
            language: "test".into(),
            github_repo: "owner/repo".into(),
            binary_name: indexer_name.into(),
            version: "v1.0.0".into(),
            default_args: vec!["index".into()],
            output_file: "index.scip".into(),
            install_method: InstallMethod::GitHubBinary {
                asset_pattern: format!("{indexer_name}-{{arch}}-{{os}}"),
            },
            backend_capabilities: BackendCapabilities::windows_linux_backends("test"),
        }
    }

    #[test]
    fn wsl_path_fallback_converts_drive_paths() {
        assert_eq!(
            fallback_wsl_path_for_windows_path(Path::new(r"C:\Users\alice\repo")).unwrap(),
            "/mnt/c/Users/alice/repo"
        );
        assert_eq!(
            fallback_wsl_path_for_windows_path(Path::new(r"F:\Claude\projects\sentry")).unwrap(),
            "/mnt/f/Claude/projects/sentry"
        );
        assert_eq!(
            fallback_wsl_path_for_windows_path(Path::new(r"\\?\F:\Claude\projects\sentry"))
                .unwrap(),
            "/mnt/f/Claude/projects/sentry"
        );
    }

    #[test]
    fn wsl_path_conversion_strips_extended_windows_prefix_before_wslpath() {
        assert_eq!(
            remove_windows_extended_path_prefix(Path::new(r"\\?\F:\Claude\projects\sentry")),
            PathBuf::from(r"F:\Claude\projects\sentry")
        );
        assert_eq!(
            remove_windows_extended_path_prefix(Path::new(r"\\?\UNC\server\share\project")),
            PathBuf::from(r"\\server\share\project")
        );
    }

    #[test]
    fn wsl_distro_args_are_applied_before_linux_command() {
        let mut args = Vec::new();
        push_wsl_distro_os_args(&mut args, Some("Ubuntu-24.04"));
        args.push(OsString::from("--"));
        args.push(OsString::from("wslpath"));

        assert_eq!(
            args,
            vec![
                OsString::from("-d"),
                OsString::from("Ubuntu-24.04"),
                OsString::from("--"),
                OsString::from("wslpath"),
            ]
        );
        assert_eq!(
            wsl_display_prefix(Some("Ubuntu-24.04")),
            "wsl.exe -d Ubuntu-24.04"
        );
        assert_eq!(wsl_display_prefix(None), "wsl.exe");
    }

    #[test]
    fn wsl_probe_requires_a_runnable_distro() {
        assert_eq!(
            wsl_probe_args(None),
            vec![OsString::from("--"), OsString::from("true")]
        );
        assert_eq!(
            wsl_probe_args(Some("Ubuntu-24.04")),
            vec![
                OsString::from("-d"),
                OsString::from("Ubuntu-24.04"),
                OsString::from("--"),
                OsString::from("true"),
            ]
        );
    }

    #[test]
    fn process_output_text_decodes_utf16le_wsl_messages() {
        let bytes = "Windows Subsystem for Linux has no installed distributions."
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();

        assert_eq!(
            process_output_text(&bytes),
            "Windows Subsystem for Linux has no installed distributions."
        );
    }

    #[test]
    fn docker_mount_plan_uses_stable_linux_targets() {
        let plan = docker_mount_plan(
            Path::new(r"\\?\F:\Claude\projects\sentry"),
            Path::new(r"\\?\C:\Users\alice\AppData\Local\Temp\scip-io"),
        )
        .unwrap();

        assert_eq!(plan.project_target, DOCKER_WORKSPACE);
        assert_eq!(plan.temp_target, DOCKER_TEMP);
        assert_eq!(plan.cache_target, DOCKER_CACHE);
        assert_eq!(
            plan.project_source,
            PathBuf::from(r"F:\Claude\projects\sentry")
        );
    }

    #[tokio::test]
    async fn backend_command_builder_maps_wsl_paths_without_shell_concatenation() {
        let project = Path::new(r"F:\Claude\projects\space repo");
        let output_dir = PathBuf::from(r"C:\Users\alice\AppData\Local\Temp\scip-io");
        let temp_output = output_dir.join("ruby.scip");
        let request = BackendExecutionRequest {
            native_binary: None,
            entry: &entry("scip-ruby"),
            project_root: project,
            temp_dir: &output_dir,
            output_name: "ruby.scip",
            args: vec![
                OsString::from("index"),
                OsString::from("--output"),
                temp_output.as_os_str().to_os_string(),
            ],
            preference: BackendPreference {
                kind: ExecutionBackendKind::Wsl,
                docker_image: None,
                wsl_distro: Some("Ubuntu-24.04".into()),
            },
        };

        let prepared = prepare_wsl_command(&request, Path::new(r"C:\cache\scip-ruby"))
            .await
            .unwrap();

        assert_eq!(prepared.program, PathBuf::from("wsl.exe"));
        assert!(prepared.args.contains(&OsString::from("-d")));
        assert!(prepared.args.contains(&OsString::from("Ubuntu-24.04")));
        assert!(prepared.args.contains(&OsString::from("--cd")));
        assert!(prepared.args.iter().any(|arg| {
            arg.to_string_lossy()
                .contains("/mnt/f/Claude/projects/space repo")
        }));
        assert!(
            prepared
                .args
                .iter()
                .any(|arg| arg.to_string_lossy().contains("ruby.scip"))
        );
        assert_eq!(prepared.output_path_on_host, temp_output);
    }

    #[test]
    fn backend_command_builder_maps_docker_mounts_and_output_path() {
        let temp = TempDir::new().unwrap();
        let project = Path::new(r"F:\Claude\projects\sentry");
        let cache_binary = install_dir()
            .join("linux-backends")
            .join("scip-ruby")
            .join("v1.0.0")
            .join("linux-x86_64")
            .join("scip-ruby");
        let request = BackendExecutionRequest {
            native_binary: None,
            entry: &entry("scip-ruby"),
            project_root: project,
            temp_dir: temp.path(),
            output_name: "ruby.scip",
            args: vec![
                OsString::from("index"),
                OsString::from("--output"),
                temp.path().join("ruby.scip").as_os_str().to_os_string(),
            ],
            preference: BackendPreference {
                kind: ExecutionBackendKind::Docker,
                docker_image: Some("ruby-indexer:test".into()),
                wsl_distro: None,
            },
        };

        let prepared = prepare_docker_command(&request, &cache_binary).unwrap();
        let args = prepared
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert_eq!(prepared.program, PathBuf::from("docker"));
        assert!(args.contains(&"ruby-indexer:test".to_string()));
        assert!(args.iter().any(|arg| arg.contains("target=/workspace")));
        assert!(
            args.iter()
                .any(|arg| arg.contains("target=/tmp/scip-io-output"))
        );
        assert!(args.iter().any(|arg| arg.contains("target=/cache/scip-io")));
        assert!(args.contains(&"/tmp/scip-io-output/ruby.scip".to_string()));
        assert_eq!(prepared.output_path_on_host, temp.path().join("ruby.scip"));
        assert_eq!(
            prepared.output_path_for_process,
            OsString::from("/tmp/scip-io-output/ruby.scip")
        );
    }

    #[test]
    fn compile_commands_backend_preflight_rejects_windows_toolchains() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("compile_commands.json");
        std::fs::write(
            &path,
            r#"[{"directory":"F:\\Claude\\projects\\foo","command":"cl.exe /I C:\\SDK foo.cpp","file":"F:\\Claude\\projects\\foo\\foo.cpp"}]"#,
        )
        .unwrap();

        let error = preflight_compile_commands_for_linux_backend(&path).unwrap_err();

        assert!(error.to_string().contains("not Linux-compatible"));
    }

    #[test]
    fn compile_commands_backend_preflight_accepts_linux_relative_commands() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("compile_commands.json");
        std::fs::write(
            &path,
            r#"[{"directory":"/workspace","command":"clang++ -I include -c src/foo.cpp","file":"src/foo.cpp"}]"#,
        )
        .unwrap();

        preflight_compile_commands_for_linux_backend(&path).unwrap();
    }
}
