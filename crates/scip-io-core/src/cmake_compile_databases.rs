use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

pub use crate::config::{
    CmakeCompileDatabaseBuildConfig, CmakeCompileDatabaseConfig, CmakeCompileDatabasePreset,
};
use crate::indexer::backend::{
    BackendPreference, ExecutionBackendKind, probe_wsl_with_distro,
    wsl_path_for_windows_path_with_distro,
};
use crate::process::{hidden_std_command, hidden_tokio_command};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CmakeCompileDatabaseGenerationPlan {
    pub jobs: Vec<CmakeCompileDatabaseJob>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CmakeCompileDatabaseJob {
    pub name: String,
    pub cmake: PathBuf,
    pub source_dir: PathBuf,
    pub build_dir: PathBuf,
    pub compile_commands: PathBuf,
    pub args: Vec<String>,
    pub status: CmakeCompileDatabaseJobStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CmakeCompileDatabaseJobStatus {
    Pending,
    Existing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct CmakeCompileDatabaseGenerationReport {
    pub jobs: Vec<CmakeCompileDatabaseJobReport>,
    pub planned_jobs: usize,
    pub generated_jobs: usize,
    pub existing_jobs: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CmakeCompileDatabaseJobReport {
    pub name: String,
    pub source_dir: PathBuf,
    pub build_dir: PathBuf,
    pub compile_commands: PathBuf,
    pub status: CmakeCompileDatabaseJobStatus,
}

pub fn cmake_compile_database_generation_enabled(config: &CmakeCompileDatabaseConfig) -> bool {
    config.generate_compile_databases.unwrap_or(false)
}

pub fn plan_cmake_compile_database_generation(
    root: &Path,
    config: &CmakeCompileDatabaseConfig,
) -> Result<CmakeCompileDatabaseGenerationPlan> {
    if !cmake_compile_database_generation_enabled(config) {
        return Ok(CmakeCompileDatabaseGenerationPlan { jobs: Vec::new() });
    }

    let cmake = config
        .cmake
        .clone()
        .unwrap_or_else(|| PathBuf::from("cmake"));
    let default_source_dir = resolve_source_dir(root, config)?;
    let default_generator = config.generator.as_deref();
    let default_refresh = config.refresh.unwrap_or(false);
    let mut jobs = Vec::new();

    if let Some(preset) = config.preset {
        jobs.extend(preset_jobs(
            root,
            preset,
            &cmake,
            &default_source_dir,
            default_generator,
            default_refresh,
            config.build_root.as_deref(),
        ));
    }

    for build in &config.builds {
        jobs.push(custom_job(
            root,
            build,
            &cmake,
            &default_source_dir,
            default_generator,
            default_refresh,
        )?);
    }

    if jobs.is_empty() {
        bail!(
            "CMake compile database generation is enabled but no preset or custom builds were configured"
        );
    }

    Ok(CmakeCompileDatabaseGenerationPlan { jobs })
}

pub fn generate_cmake_compile_databases(
    root: &Path,
    config: &CmakeCompileDatabaseConfig,
) -> Result<CmakeCompileDatabaseGenerationReport> {
    let plan = plan_cmake_compile_database_generation(root, config)?;
    let mut report = CmakeCompileDatabaseGenerationReport {
        planned_jobs: plan.jobs.len(),
        ..CmakeCompileDatabaseGenerationReport::default()
    };

    for job in plan.jobs {
        if job.status == CmakeCompileDatabaseJobStatus::Existing {
            report.existing_jobs += 1;
            report
                .jobs
                .push(job_report(job, CmakeCompileDatabaseJobStatus::Existing));
            continue;
        }

        if let Some(parent) = job.build_dir.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        let output = hidden_std_command(&job.cmake)
            .args(&job.args)
            .current_dir(root)
            .output()
            .with_context(|| {
                format!(
                    "Failed to run CMake compile database job '{}' with {}",
                    job.name,
                    job.cmake.display()
                )
            })?;
        ensure_cmake_job_succeeded(&job, output.status, &output.stdout, &output.stderr)?;
        if !job.compile_commands.exists() {
            bail!(
                "CMake compile database job '{}' completed but did not create {}",
                job.name,
                job.compile_commands.display()
            );
        }

        report.generated_jobs += 1;
        report
            .jobs
            .push(job_report(job, CmakeCompileDatabaseJobStatus::Pending));
    }

    Ok(report)
}

pub async fn generate_cmake_compile_databases_with_backend(
    root: &Path,
    config: &CmakeCompileDatabaseConfig,
    backend_preference: &BackendPreference,
) -> Result<CmakeCompileDatabaseGenerationReport> {
    match resolve_cmake_generation_backend(backend_preference).await? {
        CmakeGenerationBackend::Native => generate_cmake_compile_databases(root, config),
        CmakeGenerationBackend::Wsl { distro } => {
            generate_cmake_compile_databases_with_wsl(root, config, distro.as_deref()).await
        }
    }
}

enum CmakeGenerationBackend {
    Native,
    Wsl { distro: Option<String> },
}

async fn resolve_cmake_generation_backend(
    backend_preference: &BackendPreference,
) -> Result<CmakeGenerationBackend> {
    match backend_preference.kind {
        ExecutionBackendKind::Native => Ok(CmakeGenerationBackend::Native),
        ExecutionBackendKind::Wsl => {
            let probe = probe_wsl_with_distro(backend_preference.wsl_distro.as_deref()).await;
            if !probe.available {
                bail!(
                    "CMake compile database generation requires WSL because scip-clang is configured for WSL, but WSL is unavailable: {}",
                    probe.detail.unwrap_or_else(|| "probe failed".into())
                );
            }
            Ok(CmakeGenerationBackend::Wsl {
                distro: backend_preference.wsl_distro.clone(),
            })
        }
        ExecutionBackendKind::Docker => {
            bail!(
                "CMake compile database generation through Docker is not supported yet; generate compile_commands.json inside the container or use WSL/native generation"
            )
        }
        ExecutionBackendKind::Auto => {
            if cfg!(windows) {
                let probe = probe_wsl_with_distro(backend_preference.wsl_distro.as_deref()).await;
                if !probe.available {
                    bail!(
                        "CMake compile database generation on Windows needs WSL so generated paths match the scip-clang Linux backend: {}",
                        probe.detail.unwrap_or_else(|| "probe failed".into())
                    );
                }
                Ok(CmakeGenerationBackend::Wsl {
                    distro: backend_preference.wsl_distro.clone(),
                })
            } else {
                Ok(CmakeGenerationBackend::Native)
            }
        }
        ExecutionBackendKind::Disabled => {
            bail!("CMake compile database generation is disabled by backend configuration")
        }
    }
}

async fn generate_cmake_compile_databases_with_wsl(
    root: &Path,
    config: &CmakeCompileDatabaseConfig,
    distro: Option<&str>,
) -> Result<CmakeCompileDatabaseGenerationReport> {
    let plan = plan_cmake_compile_database_generation(root, config)?;
    let root_wsl = wsl_path_for_windows_path_with_distro(root, distro).await?;
    let mut report = CmakeCompileDatabaseGenerationReport {
        planned_jobs: plan.jobs.len(),
        ..CmakeCompileDatabaseGenerationReport::default()
    };

    for job in plan.jobs {
        if job.status == CmakeCompileDatabaseJobStatus::Existing {
            report.existing_jobs += 1;
            report
                .jobs
                .push(job_report(job, CmakeCompileDatabaseJobStatus::Existing));
            continue;
        }

        if let Some(parent) = job.build_dir.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        let mut command = hidden_tokio_command("wsl.exe");
        command.args(wsl_cmake_command_args(distro, &root_wsl, &job));
        let output = command.output().await.with_context(|| {
            format!(
                "Failed to run WSL CMake compile database job '{}'",
                job.name
            )
        })?;
        ensure_cmake_job_succeeded(&job, output.status, &output.stdout, &output.stderr)?;
        if !job.compile_commands.exists() {
            bail!(
                "WSL CMake compile database job '{}' completed but did not create {}",
                job.name,
                job.compile_commands.display()
            );
        }

        report.generated_jobs += 1;
        report
            .jobs
            .push(job_report(job, CmakeCompileDatabaseJobStatus::Pending));
    }

    Ok(report)
}

fn resolve_source_dir(root: &Path, config: &CmakeCompileDatabaseConfig) -> Result<PathBuf> {
    if let Some(source_dir) = &config.source_dir {
        return Ok(resolve_under_root(root, source_dir));
    }
    let llvm_source = root.join("llvm");
    if llvm_source.join("CMakeLists.txt").exists() {
        return Ok(llvm_source);
    }
    if root.join("CMakeLists.txt").exists() {
        return Ok(root.to_path_buf());
    }
    bail!(
        "CMake compile database generation needs a CMake source directory; expected {} or {}",
        root.join("llvm").join("CMakeLists.txt").display(),
        root.join("CMakeLists.txt").display()
    );
}

fn preset_jobs(
    root: &Path,
    preset: CmakeCompileDatabasePreset,
    cmake: &Path,
    source_dir: &Path,
    generator: Option<&str>,
    refresh: bool,
    build_root: Option<&Path>,
) -> Vec<CmakeCompileDatabaseJob> {
    match preset {
        CmakeCompileDatabasePreset::LlvmBroad => {
            llvm_broad_jobs(root, cmake, source_dir, generator, refresh, build_root)
        }
    }
}

fn llvm_broad_jobs(
    root: &Path,
    cmake: &Path,
    source_dir: &Path,
    generator: Option<&str>,
    refresh: bool,
    build_root: Option<&Path>,
) -> Vec<CmakeCompileDatabaseJob> {
    [
        (
            "llvm-all-targets",
            vec![
                "-DLLVM_TARGETS_TO_BUILD=all",
                "-DLLVM_ENABLE_PROJECTS=",
                "-DLLVM_ENABLE_RUNTIMES=",
                "-DLLVM_INCLUDE_TESTS=ON",
                "-DLLVM_BUILD_TOOLS=ON",
                "-DLLVM_BUILD_EXAMPLES=OFF",
            ],
        ),
        (
            "llvm-projects",
            vec![
                "-DLLVM_TARGETS_TO_BUILD=all",
                "-DLLVM_ENABLE_PROJECTS=clang;clang-tools-extra;mlir;lld;lldb;flang;polly;bolt",
                "-DLLVM_ENABLE_RUNTIMES=",
                "-DLLVM_INCLUDE_TESTS=ON",
                "-DLLVM_BUILD_TOOLS=ON",
                "-DLLVM_BUILD_EXAMPLES=OFF",
            ],
        ),
        (
            "llvm-runtimes",
            vec![
                "-DLLVM_TARGETS_TO_BUILD=all",
                "-DLLVM_ENABLE_PROJECTS=clang",
                "-DLLVM_ENABLE_RUNTIMES=compiler-rt;libc;libcxx;libcxxabi;libunwind;openmp;offload",
                "-DLLVM_INCLUDE_TESTS=ON",
                "-DLLVM_BUILD_TOOLS=ON",
                "-DLLVM_BUILD_EXAMPLES=OFF",
            ],
        ),
    ]
    .into_iter()
    .map(|(name, definitions)| {
        let build_dir = preset_build_dir(root, name, build_root);
        build_job(BuildJobSpec {
            root,
            name,
            cmake,
            source_dir,
            build_dir,
            generator,
            refresh,
            extra_args: definitions.into_iter().map(String::from).collect(),
        })
    })
    .collect()
}

fn custom_job(
    root: &Path,
    build: &CmakeCompileDatabaseBuildConfig,
    cmake: &Path,
    default_source_dir: &Path,
    default_generator: Option<&str>,
    default_refresh: bool,
) -> Result<CmakeCompileDatabaseJob> {
    let build_dir = resolve_under_root(root, &build.build_dir);
    let source_dir = build
        .source_dir
        .as_ref()
        .map(|path| resolve_under_root(root, path))
        .unwrap_or_else(|| default_source_dir.to_path_buf());
    let name = build
        .name
        .clone()
        .or_else(|| {
            build_dir
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "cmake-build".to_string());

    Ok(build_job(BuildJobSpec {
        root,
        name: &name,
        cmake,
        source_dir: &source_dir,
        build_dir,
        generator: build.generator.as_deref().or(default_generator),
        refresh: build.refresh.unwrap_or(default_refresh),
        extra_args: build.args.clone(),
    }))
}

struct BuildJobSpec<'a> {
    root: &'a Path,
    name: &'a str,
    cmake: &'a Path,
    source_dir: &'a Path,
    build_dir: PathBuf,
    generator: Option<&'a str>,
    refresh: bool,
    extra_args: Vec<String>,
}

fn build_job(spec: BuildJobSpec<'_>) -> CmakeCompileDatabaseJob {
    let compile_commands = spec.build_dir.join("compile_commands.json");
    let mut args = vec![
        "-S".to_string(),
        display_path_arg(spec.root, spec.source_dir),
        "-B".to_string(),
        display_path_arg(spec.root, &spec.build_dir),
    ];
    if let Some(generator) = spec.generator {
        args.push("-G".to_string());
        args.push(generator.to_string());
    }
    args.push("-DCMAKE_EXPORT_COMPILE_COMMANDS=ON".to_string());
    args.extend(spec.extra_args);
    let status = if compile_commands.exists() && !spec.refresh {
        CmakeCompileDatabaseJobStatus::Existing
    } else {
        CmakeCompileDatabaseJobStatus::Pending
    };

    CmakeCompileDatabaseJob {
        name: spec.name.to_string(),
        cmake: spec.cmake.to_path_buf(),
        source_dir: spec.source_dir.to_path_buf(),
        build_dir: spec.build_dir,
        compile_commands,
        args,
        status,
    }
}

fn preset_build_dir(root: &Path, name: &str, build_root: Option<&Path>) -> PathBuf {
    match build_root {
        Some(build_root) => resolve_under_root(root, build_root).join(name),
        None => root.join(format!("build-scip-io-{name}")),
    }
}

fn resolve_under_root(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn display_path_arg(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn job_report(
    job: CmakeCompileDatabaseJob,
    status: CmakeCompileDatabaseJobStatus,
) -> CmakeCompileDatabaseJobReport {
    CmakeCompileDatabaseJobReport {
        name: job.name,
        source_dir: job.source_dir,
        build_dir: job.build_dir,
        compile_commands: job.compile_commands,
        status,
    }
}

fn wsl_cmake_command_args(
    distro: Option<&str>,
    root_wsl: &str,
    job: &CmakeCompileDatabaseJob,
) -> Vec<OsString> {
    let mut args = Vec::new();
    if let Some(distro) = distro {
        args.push(OsString::from("-d"));
        args.push(OsString::from(distro));
    }
    args.push(OsString::from("--cd"));
    args.push(OsString::from(root_wsl));
    args.push(OsString::from("--exec"));
    args.push(job.cmake.as_os_str().to_os_string());
    args.extend(job.args.iter().map(OsString::from));
    args
}

fn ensure_cmake_job_succeeded(
    job: &CmakeCompileDatabaseJob,
    status: std::process::ExitStatus,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<()> {
    if status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(stderr);
    let stdout = String::from_utf8_lossy(stdout);
    bail!(
        "CMake compile database job '{}' failed with status {}. stdout: {} stderr: {}",
        job.name,
        status,
        bounded_output(&stdout),
        bounded_output(&stderr)
    );
}

fn bounded_output(output: &str) -> String {
    const LIMIT: usize = 4000;
    if output.len() <= LIMIT {
        return output.trim().to_string();
    }
    format!("{}... [truncated]", output[..LIMIT].trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_cmake_args_run_cmake_from_linux_repo_root() {
        let job = CmakeCompileDatabaseJob {
            name: "llvm-all-targets".to_string(),
            cmake: PathBuf::from("cmake"),
            source_dir: PathBuf::from(r"F:\repo\llvm"),
            build_dir: PathBuf::from(r"F:\repo\build-scip-io-llvm-all-targets"),
            compile_commands: PathBuf::from(
                r"F:\repo\build-scip-io-llvm-all-targets\compile_commands.json",
            ),
            args: vec![
                "-S".to_string(),
                "llvm".to_string(),
                "-B".to_string(),
                "build-scip-io-llvm-all-targets".to_string(),
                "-DCMAKE_EXPORT_COMPILE_COMMANDS=ON".to_string(),
                "-DLLVM_ENABLE_RUNTIMES=compiler-rt;libc;libcxx".to_string(),
            ],
            status: CmakeCompileDatabaseJobStatus::Pending,
        };

        let args = wsl_cmake_command_args(Some("Ubuntu-24.04"), "/mnt/f/repo", &job);

        assert_eq!(
            args,
            vec![
                OsString::from("-d"),
                OsString::from("Ubuntu-24.04"),
                OsString::from("--cd"),
                OsString::from("/mnt/f/repo"),
                OsString::from("--exec"),
                OsString::from("cmake"),
                OsString::from("-S"),
                OsString::from("llvm"),
                OsString::from("-B"),
                OsString::from("build-scip-io-llvm-all-targets"),
                OsString::from("-DCMAKE_EXPORT_COMPILE_COMMANDS=ON"),
                OsString::from("-DLLVM_ENABLE_RUNTIMES=compiler-rt;libc;libcxx"),
            ]
        );
    }
}
