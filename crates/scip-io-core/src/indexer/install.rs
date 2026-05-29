use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde::Deserialize;

use crate::indexer::version::normalize_version;
use crate::indexer::{IndexerEntry, InstallMethod, install_dir, npm_package_dir};
use crate::progress::{ProgressEvent, ProgressHandler};

// ---------------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
fn platform_os() -> &'static str {
    IndexerAssetPlatform::Host.os()
}

#[cfg(test)]
fn platform_arch() -> &'static str {
    IndexerAssetPlatform::Host.arch()
}

/// Goreleaser-style arch names (used by scip-go).
#[cfg(test)]
fn goreleaser_arch() -> &'static str {
    IndexerAssetPlatform::Host.goreleaser_arch()
}

/// Rust target triple for the current platform.
#[cfg(test)]
fn target_triple() -> &'static str {
    IndexerAssetPlatform::Host.target_triple()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexerAssetPlatform {
    Host,
    LinuxX86_64,
    LinuxAarch64,
}

impl IndexerAssetPlatform {
    fn os(self) -> &'static str {
        match self {
            Self::LinuxX86_64 | Self::LinuxAarch64 => "linux",
            Self::Host if cfg!(target_os = "linux") => "linux",
            Self::Host if cfg!(target_os = "macos") => "darwin",
            Self::Host if cfg!(target_os = "windows") => "windows",
            Self::Host => "unknown",
        }
    }

    fn ext(self) -> &'static str {
        match self {
            Self::Host if cfg!(target_os = "windows") => ".exe",
            _ => "",
        }
    }

    fn arch(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "x86_64",
            Self::LinuxAarch64 => "aarch64",
            Self::Host if cfg!(target_arch = "x86_64") => "x86_64",
            Self::Host if cfg!(target_arch = "aarch64") => "aarch64",
            Self::Host => "unknown",
        }
    }

    fn goreleaser_arch(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "amd64",
            Self::LinuxAarch64 => "arm64",
            Self::Host if cfg!(target_arch = "x86_64") => "amd64",
            Self::Host if cfg!(target_arch = "aarch64") => "arm64",
            Self::Host => "unknown",
        }
    }

    fn target_triple(self) -> &'static str {
        match self {
            Self::LinuxX86_64 => "x86_64-unknown-linux-gnu",
            Self::LinuxAarch64 => "aarch64-unknown-linux-gnu",
            Self::Host if cfg!(all(target_os = "linux", target_arch = "x86_64")) => {
                "x86_64-unknown-linux-gnu"
            }
            Self::Host if cfg!(all(target_os = "linux", target_arch = "aarch64")) => {
                "aarch64-unknown-linux-gnu"
            }
            Self::Host if cfg!(all(target_os = "macos", target_arch = "x86_64")) => {
                "x86_64-apple-darwin"
            }
            Self::Host if cfg!(all(target_os = "macos", target_arch = "aarch64")) => {
                "aarch64-apple-darwin"
            }
            Self::Host if cfg!(all(target_os = "windows", target_arch = "x86_64")) => {
                "x86_64-pc-windows-msvc"
            }
            Self::Host if cfg!(all(target_os = "windows", target_arch = "aarch64")) => {
                "aarch64-pc-windows-msvc"
            }
            Self::Host => "unknown-unknown-unknown",
        }
    }
}

impl std::fmt::Display for IndexerAssetPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host => write!(f, "host"),
            Self::LinuxX86_64 => write!(f, "linux-x86_64"),
            Self::LinuxAarch64 => write!(f, "linux-aarch64"),
        }
    }
}

fn platform_os_for(platform: IndexerAssetPlatform) -> &'static str {
    platform.os()
}

fn platform_ext_for(platform: IndexerAssetPlatform) -> &'static str {
    platform.ext()
}

fn platform_arch_for(platform: IndexerAssetPlatform) -> &'static str {
    platform.arch()
}

fn goreleaser_arch_for(platform: IndexerAssetPlatform) -> &'static str {
    platform.goreleaser_arch()
}

fn target_triple_for(platform: IndexerAssetPlatform) -> &'static str {
    platform.target_triple()
}

/// Resolve placeholders in an asset pattern string.
#[cfg(test)]
fn resolve_pattern(pattern: &str, version: &str) -> String {
    resolve_pattern_for_platform(pattern, version, IndexerAssetPlatform::Host)
}

fn resolve_pattern_for_platform(
    pattern: &str,
    version: &str,
    platform: IndexerAssetPlatform,
) -> String {
    pattern
        .replace("{version}", version)
        .replace("{os}", platform_os_for(platform))
        .replace("{arch}", platform_arch_for(platform))
        .replace("{target_triple}", target_triple_for(platform))
        .replace("{goreleaser_arch}", goreleaser_arch_for(platform))
        .replace("{ext}", platform_ext_for(platform))
}

/// Build the full GitHub release download URL.
fn github_release_url(github_repo: &str, version: &str, asset: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/{}/{}",
        github_repo, version, asset,
    )
}

async fn latest_github_release_tag(github_repo: &str) -> Result<String> {
    let url = format!("https://api.github.com/repos/{github_repo}/releases/latest");
    let response = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", "scip-io")
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .with_context(|| format!("Failed to check latest release for {github_repo}"))?;

    if !response.status().is_success() {
        bail!(
            "GitHub latest release check failed for {}: HTTP {}",
            github_repo,
            response.status()
        );
    }

    let json = response
        .json::<serde_json::Value>()
        .await
        .with_context(|| format!("Failed to parse latest release for {github_repo}"))?;
    json["tag_name"]
        .as_str()
        .map(str::to_owned)
        .filter(|tag| !tag.trim().is_empty())
        .with_context(|| format!("Latest release for {github_repo} has no tag_name"))
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubAsset {
    name: String,
}

async fn github_releases(github_repo: &str) -> Result<Vec<GitHubRelease>> {
    let url = format!("https://api.github.com/repos/{github_repo}/releases?per_page=20");
    let response = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", "scip-io")
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .with_context(|| format!("Failed to check releases for {github_repo}"))?;

    if !response.status().is_success() {
        bail!(
            "GitHub release check failed for {}: HTTP {}",
            github_repo,
            response.status()
        );
    }

    response
        .json::<Vec<GitHubRelease>>()
        .await
        .with_context(|| format!("Failed to parse releases for {github_repo}"))
}

async fn github_release_by_tag(github_repo: &str, tag: &str) -> Result<GitHubRelease> {
    let url = format!("https://api.github.com/repos/{github_repo}/releases/tags/{tag}");
    let response = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", "scip-io")
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .with_context(|| format!("Failed to check release {tag} for {github_repo}"))?;

    if !response.status().is_success() {
        bail!(
            "GitHub release check failed for {} {}: HTTP {}",
            github_repo,
            tag,
            response.status()
        );
    }

    response
        .json::<GitHubRelease>()
        .await
        .with_context(|| format!("Failed to parse release {tag} for {github_repo}"))
}

async fn latest_compatible_github_release_tag(entry: &IndexerEntry) -> Result<String> {
    latest_compatible_github_release_tag_for_platform(entry, IndexerAssetPlatform::Host).await
}

async fn latest_compatible_github_release_tag_for_platform(
    entry: &IndexerEntry,
    platform: IndexerAssetPlatform,
) -> Result<String> {
    let releases = github_releases(&entry.github_repo).await?;
    first_compatible_github_release_tag_for_platform(entry, &releases, platform)
}

#[cfg(test)]
fn first_compatible_github_release_tag(
    entry: &IndexerEntry,
    releases: &[GitHubRelease],
) -> Result<String> {
    first_compatible_github_release_tag_for_platform(entry, releases, IndexerAssetPlatform::Host)
}

fn first_compatible_github_release_tag_for_platform(
    entry: &IndexerEntry,
    releases: &[GitHubRelease],
    platform: IndexerAssetPlatform,
) -> Result<String> {
    for release in releases {
        let tag = release.tag_name.trim();
        if tag.is_empty() || release.draft || release.prerelease || is_moving_release_tag(tag) {
            continue;
        }

        let expected_assets = expected_github_assets_for_platform(entry, tag, platform)?;
        if expected_assets.is_empty() {
            continue;
        }
        if release
            .assets
            .iter()
            .any(|asset| expected_assets.contains(&asset.name))
        {
            return Ok(tag.to_owned());
        }
    }

    bail!(
        "No compatible {} release found for {}",
        entry.indexer_name,
        platform
    )
}

fn is_moving_release_tag(tag: &str) -> bool {
    let tag = tag.trim().to_ascii_lowercase();
    matches!(
        tag.as_str(),
        "nightly" | "latest" | "main" | "master" | "snapshot"
    )
}

pub fn expected_github_assets_for_platform(
    entry: &IndexerEntry,
    version: &str,
    platform: IndexerAssetPlatform,
) -> Result<Vec<String>> {
    match &entry.install_method {
        InstallMethod::GitHubBinary { asset_pattern }
        | InstallMethod::GitHubGz { asset_pattern }
        | InstallMethod::GitHubTarGz { asset_pattern, .. }
        | InstallMethod::GitHubZip { asset_pattern, .. } => Ok(
            pattern_asset_candidates_for_platform(asset_pattern, version, platform),
        ),
        InstallMethod::GitHubLauncher {
            unix_asset,
            windows_asset,
        } => Ok(pattern_asset_candidates_for_platform(
            if platform == IndexerAssetPlatform::Host && cfg!(windows) {
                windows_asset
            } else {
                unix_asset
            },
            version,
            platform,
        )),
        InstallMethod::Npm { .. } | InstallMethod::DotnetTool { .. } => Ok(Vec::new()),
        InstallMethod::CoveredBy {
            indexer_name,
            reason,
        } => {
            bail!(
                "Indexer '{}' is covered by '{}': {}",
                entry.indexer_name,
                indexer_name,
                reason
            )
        }
        InstallMethod::Unsupported { reason } => {
            bail!(
                "Indexer '{}' cannot be automatically installed: {}",
                entry.indexer_name,
                reason
            )
        }
    }
}

fn version_pattern_candidates(version: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let raw = version.trim();
    if !raw.is_empty() {
        candidates.push(raw.to_owned());
    }

    let normalized = normalize_version(version);
    if !normalized.is_empty() && candidates.iter().all(|candidate| candidate != &normalized) {
        candidates.push(normalized);
    }

    candidates
}

fn pattern_asset_candidates(pattern: &str, version: &str) -> Vec<String> {
    pattern_asset_candidates_for_platform(pattern, version, IndexerAssetPlatform::Host)
}

fn pattern_asset_candidates_for_platform(
    pattern: &str,
    version: &str,
    platform: IndexerAssetPlatform,
) -> Vec<String> {
    let mut assets = Vec::new();
    for candidate in version_pattern_candidates(version) {
        let asset = resolve_pattern_for_platform(pattern, &candidate, platform);
        if !assets.contains(&asset) {
            assets.push(asset);
        }
    }
    assets
}

async fn select_github_release_asset(
    github_repo: &str,
    release_tag: &str,
    candidates: Vec<String>,
) -> Result<String> {
    let release = github_release_by_tag(github_repo, release_tag).await?;
    for candidate in &candidates {
        if release.assets.iter().any(|asset| asset.name == *candidate) {
            return Ok(candidate.clone());
        }
    }

    bail!(
        "Release {} for {} does not contain a compatible asset for this platform. Expected one of: {}",
        release_tag,
        github_repo,
        candidates.join(", ")
    )
}

async fn latest_npm_package_version(package: &str) -> Result<String> {
    let npm = which::which("npm")
        .context("npm not found on PATH. Install Node.js to use this indexer, or install the indexer manually.")?;
    let output = tokio::process::Command::new(&npm)
        .args(["view", package, "version", "--silent"])
        .output()
        .await
        .context("Failed to run npm view")?;

    if !output.status.success() {
        bail!("npm view failed with {}", output.status);
    }

    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if version.is_empty() {
        bail!("npm returned no latest version for {package}");
    }
    Ok(version)
}

pub async fn resolve_latest_compatible_version(entry: &IndexerEntry) -> Result<String> {
    match &entry.install_method {
        InstallMethod::Npm { package } => latest_npm_package_version(package).await,
        InstallMethod::DotnetTool { .. } => Ok(normalize_version(
            &latest_github_release_tag(&entry.github_repo).await?,
        )),
        InstallMethod::GitHubBinary { .. }
        | InstallMethod::GitHubGz { .. }
        | InstallMethod::GitHubTarGz { .. }
        | InstallMethod::GitHubZip { .. }
        | InstallMethod::GitHubLauncher { .. } => latest_compatible_github_release_tag(entry).await,
        InstallMethod::CoveredBy {
            indexer_name,
            reason,
        } => {
            bail!(
                "Indexer '{}' is covered by '{}': {}",
                entry.indexer_name,
                indexer_name,
                reason
            )
        }
        InstallMethod::Unsupported { reason } => {
            bail!(
                "Indexer '{}' cannot be automatically installed: {}",
                entry.indexer_name,
                reason
            )
        }
    }
}

pub async fn resolve_latest_compatible_version_for_platform(
    entry: &IndexerEntry,
    platform: IndexerAssetPlatform,
) -> Result<String> {
    match &entry.install_method {
        InstallMethod::Npm { package } if platform == IndexerAssetPlatform::Host => {
            latest_npm_package_version(package).await
        }
        InstallMethod::DotnetTool { .. } if platform == IndexerAssetPlatform::Host => Ok(
            normalize_version(&latest_github_release_tag(&entry.github_repo).await?),
        ),
        InstallMethod::GitHubBinary { .. }
        | InstallMethod::GitHubGz { .. }
        | InstallMethod::GitHubTarGz { .. }
        | InstallMethod::GitHubZip { .. }
        | InstallMethod::GitHubLauncher { .. } => {
            latest_compatible_github_release_tag_for_platform(entry, platform).await
        }
        InstallMethod::Npm { .. } | InstallMethod::DotnetTool { .. } => {
            bail!(
                "{} does not expose downloadable {} assets",
                entry.indexer_name,
                platform
            )
        }
        InstallMethod::CoveredBy {
            indexer_name,
            reason,
        } => {
            bail!(
                "Indexer '{}' is covered by '{}': {}",
                entry.indexer_name,
                indexer_name,
                reason
            )
        }
        InstallMethod::Unsupported { reason } => {
            bail!(
                "Indexer '{}' cannot be automatically installed: {}",
                entry.indexer_name,
                reason
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Core download helper
// ---------------------------------------------------------------------------

/// Download a URL to a local file, reporting progress events.
async fn download_to_file(
    url: &str,
    dest: &Path,
    indexer_name: &str,
    progress: &dyn ProgressHandler,
) -> Result<()> {
    tracing::info!(%url, "downloading");

    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .header("User-Agent", "scip-io")
        .send()
        .await
        .with_context(|| format!("Failed to download {url}"))?;

    if !response.status().is_success() {
        bail!("Download failed: HTTP {} for {}", response.status(), url);
    }

    let total_size = response.content_length();
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("Cannot create {}", dest.display()))?;

    let mut downloaded: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Error reading download stream")?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk).await?;
        downloaded += chunk.len() as u64;
        progress.on_event(ProgressEvent::DownloadProgress {
            indexer: indexer_name.to_owned(),
            bytes: downloaded,
            total: total_size,
        });
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Archive extraction (sync -- called via spawn_blocking)
// ---------------------------------------------------------------------------

/// Decompress a .gz file (single file, not tarball) to dest.
fn extract_gz(gz_path: &Path, dest: &Path) -> Result<()> {
    let file = std::fs::File::open(gz_path)
        .with_context(|| format!("Cannot open {}", gz_path.display()))?;
    let mut decoder = flate2::read::GzDecoder::new(file);
    let mut out =
        std::fs::File::create(dest).with_context(|| format!("Cannot create {}", dest.display()))?;
    io::copy(&mut decoder, &mut out)?;
    Ok(())
}

/// Extract a specific binary from a .tar.gz archive.
fn extract_tar_gz(
    archive_path: &Path,
    dest_dir: &Path,
    binary_name: &str,
    binary_path_in_archive: Option<&str>,
) -> Result<PathBuf> {
    let file = std::fs::File::open(archive_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    let target = binary_path_in_archive.unwrap_or(binary_name);
    let windows_binary_name = if cfg!(windows) && !binary_name.ends_with(".exe") {
        Some(format!("{binary_name}.exe"))
    } else {
        None
    };

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let path_str = path.to_string_lossy();

        // Match either the exact path or just the filename component
        let matches = path_str == target
            || windows_binary_name
                .as_deref()
                .is_some_and(|windows_name| path_str == windows_name)
            || path.file_name().is_some_and(|file_name| {
                file_name == binary_name
                    || windows_binary_name
                        .as_deref()
                        .is_some_and(|windows_name| file_name == windows_name)
            });

        if matches {
            let dest_name = if cfg!(windows) && !binary_name.ends_with(".exe") {
                format!("{binary_name}.exe")
            } else {
                binary_name.to_owned()
            };
            let dest = dest_dir.join(dest_name);
            let mut out = std::fs::File::create(&dest)?;
            io::copy(&mut entry, &mut out)?;
            return Ok(dest);
        }
    }

    bail!(
        "Binary \'{}\' not found in archive {}",
        target,
        archive_path.display()
    )
}

/// Extract a specific binary from a .zip archive.
fn extract_zip(
    archive_path: &Path,
    dest_dir: &Path,
    binary_name: &str,
    binary_path_in_archive: Option<&str>,
) -> Result<PathBuf> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    let target = binary_path_in_archive.unwrap_or(binary_name);

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let path_str = entry.name().to_owned();

        let matches = path_str == target
            || Path::new(&path_str)
                .file_name()
                .is_some_and(|f| f == binary_name);

        if matches {
            let dest_name = if cfg!(windows) && !binary_name.ends_with(".exe") {
                format!("{}.exe", binary_name)
            } else {
                binary_name.to_owned()
            };
            let dest = dest_dir.join(&dest_name);
            let mut out = std::fs::File::create(&dest)?;
            io::copy(&mut entry, &mut out)?;
            return Ok(dest);
        }
    }

    bail!(
        "Binary \'{}\' not found in zip {}",
        target,
        archive_path.display()
    )
}

/// Set executable permission on unix.
#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Per-method install functions
// ---------------------------------------------------------------------------

async fn install_github_binary(
    entry: &IndexerEntry,
    pattern: &str,
    progress: &dyn ProgressHandler,
) -> Result<PathBuf> {
    let asset = select_github_release_asset(
        &entry.github_repo,
        &entry.version,
        pattern_asset_candidates(pattern, &entry.version),
    )
    .await?;
    let url = github_release_url(&entry.github_repo, &entry.version, &asset);

    let dir = install_dir();
    let dest_name = if cfg!(windows) {
        format!("{}.exe", entry.binary_name)
    } else {
        entry.binary_name.clone()
    };
    let dest = dir.join(&dest_name);
    let tmp = dest.with_extension("tmp");

    download_to_file(&url, &tmp, &entry.indexer_name, progress).await?;
    set_executable(&tmp)?;
    std::fs::rename(&tmp, &dest)?;

    Ok(dest)
}

pub async fn download_github_binary_for_platform(
    entry: &IndexerEntry,
    version: &str,
    platform: IndexerAssetPlatform,
    dest_dir: &Path,
    progress: &dyn ProgressHandler,
) -> Result<PathBuf> {
    let asset_pattern = match &entry.install_method {
        InstallMethod::GitHubBinary { asset_pattern } => asset_pattern,
        InstallMethod::GitHubLauncher {
            unix_asset,
            windows_asset,
        } => {
            if platform == IndexerAssetPlatform::Host && cfg!(windows) {
                windows_asset
            } else {
                unix_asset
            }
        }
        _ => {
            bail!(
                "{} does not expose a direct GitHub release asset for backend execution",
                entry.indexer_name
            );
        }
    };

    let asset = select_github_release_asset(
        &entry.github_repo,
        version,
        pattern_asset_candidates_for_platform(asset_pattern, version, platform),
    )
    .await?;
    let url = github_release_url(&entry.github_repo, version, &asset);

    std::fs::create_dir_all(dest_dir)
        .with_context(|| format!("Cannot create {}", dest_dir.display()))?;
    let dest = dest_dir.join(&entry.binary_name);
    let tmp = dest.with_extension("tmp");
    download_to_file(&url, &tmp, &entry.indexer_name, progress).await?;
    set_executable(&tmp)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

async fn install_github_gz(
    entry: &IndexerEntry,
    pattern: &str,
    progress: &dyn ProgressHandler,
) -> Result<PathBuf> {
    let asset = select_github_release_asset(
        &entry.github_repo,
        &entry.version,
        pattern_asset_candidates(pattern, &entry.version),
    )
    .await?;
    let url = github_release_url(&entry.github_repo, &entry.version, &asset);

    let dir = install_dir();
    let gz_tmp = dir.join(format!("{}.gz.tmp", entry.binary_name));
    download_to_file(&url, &gz_tmp, &entry.indexer_name, progress).await?;

    let dest_name = if cfg!(windows) {
        format!("{}.exe", entry.binary_name)
    } else {
        entry.binary_name.clone()
    };
    let dest = dir.join(&dest_name);

    let gz_path = gz_tmp.clone();
    let dest_clone = dest.clone();
    tokio::task::spawn_blocking(move || extract_gz(&gz_path, &dest_clone))
        .await
        .context("extract_gz task panicked")??;

    set_executable(&dest)?;
    std::fs::remove_file(&gz_tmp).ok();

    Ok(dest)
}

async fn install_github_tar_gz(
    entry: &IndexerEntry,
    pattern: &str,
    binary_path_in_archive: Option<&str>,
    progress: &dyn ProgressHandler,
) -> Result<PathBuf> {
    let asset = select_github_release_asset(
        &entry.github_repo,
        &entry.version,
        pattern_asset_candidates(pattern, &entry.version),
    )
    .await?;
    let url = github_release_url(&entry.github_repo, &entry.version, &asset);

    let dir = install_dir();
    let tar_tmp = dir.join(format!("{}.tar.gz.tmp", entry.binary_name));
    download_to_file(&url, &tar_tmp, &entry.indexer_name, progress).await?;

    let tar_path = tar_tmp.clone();
    let dest_dir = dir.clone();
    let bin_name = entry.binary_name.clone();
    let archive_path = binary_path_in_archive.map(|s| s.to_owned());

    let dest = tokio::task::spawn_blocking(move || {
        extract_tar_gz(&tar_path, &dest_dir, &bin_name, archive_path.as_deref())
    })
    .await
    .context("extract_tar_gz task panicked")??;

    set_executable(&dest)?;
    std::fs::remove_file(&tar_tmp).ok();

    Ok(dest)
}

async fn install_github_zip(
    entry: &IndexerEntry,
    pattern: &str,
    binary_path_in_archive: Option<&str>,
    progress: &dyn ProgressHandler,
) -> Result<PathBuf> {
    let asset = select_github_release_asset(
        &entry.github_repo,
        &entry.version,
        pattern_asset_candidates(pattern, &entry.version),
    )
    .await?;
    let url = github_release_url(&entry.github_repo, &entry.version, &asset);

    let dir = install_dir();
    let zip_tmp = dir.join(format!("{}.zip.tmp", entry.binary_name));
    download_to_file(&url, &zip_tmp, &entry.indexer_name, progress).await?;

    let zip_path = zip_tmp.clone();
    let dest_dir = dir.clone();
    let bin_name = entry.binary_name.clone();
    let archive_path = binary_path_in_archive.map(|s| s.to_owned());

    let dest = tokio::task::spawn_blocking(move || {
        extract_zip(&zip_path, &dest_dir, &bin_name, archive_path.as_deref())
    })
    .await
    .context("extract_zip task panicked")??;

    std::fs::remove_file(&zip_tmp).ok();

    Ok(dest)
}

async fn install_github_launcher(
    entry: &IndexerEntry,
    unix_asset: &str,
    windows_asset: &str,
    progress: &dyn ProgressHandler,
) -> Result<PathBuf> {
    if cfg!(windows) {
        return install_windows_github_launcher(entry, unix_asset, windows_asset, progress).await;
    }

    let pattern = if cfg!(windows) {
        windows_asset
    } else {
        unix_asset
    };
    let asset = select_github_release_asset(
        &entry.github_repo,
        &entry.version,
        pattern_asset_candidates(pattern, &entry.version),
    )
    .await?;
    let url = github_release_url(&entry.github_repo, &entry.version, &asset);

    let dir = install_dir();
    let dest_name = if cfg!(windows) {
        format!("{}.bat", entry.binary_name)
    } else {
        entry.binary_name.clone()
    };
    let dest = dir.join(&dest_name);
    let tmp = dest.with_extension("launcher.tmp");

    download_to_file(&url, &tmp, &entry.indexer_name, progress).await?;
    set_executable(&tmp)?;
    std::fs::rename(&tmp, &dest)?;

    Ok(dest)
}

async fn install_windows_github_launcher(
    entry: &IndexerEntry,
    companion_asset_pattern: &str,
    launcher_asset_pattern: &str,
    progress: &dyn ProgressHandler,
) -> Result<PathBuf> {
    let launcher_asset = select_github_release_asset(
        &entry.github_repo,
        &entry.version,
        pattern_asset_candidates(launcher_asset_pattern, &entry.version),
    )
    .await?;
    let companion_asset = select_github_release_asset(
        &entry.github_repo,
        &entry.version,
        pattern_asset_candidates(companion_asset_pattern, &entry.version),
    )
    .await?;

    let dir = install_dir();
    let launcher_dest = dir.join(format!("{}.bat", entry.binary_name));
    let companion_dest = dir.join(&entry.binary_name);
    let launcher_tmp = launcher_dest.with_extension("bat.launcher.tmp");
    let companion_tmp = companion_dest.with_extension("launcher-payload.tmp");

    let launcher_url = github_release_url(&entry.github_repo, &entry.version, &launcher_asset);
    let companion_url = github_release_url(&entry.github_repo, &entry.version, &companion_asset);

    download_to_file(
        &companion_url,
        &companion_tmp,
        &entry.indexer_name,
        progress,
    )
    .await?;
    download_to_file(&launcher_url, &launcher_tmp, &entry.indexer_name, progress).await?;
    set_executable(&launcher_tmp)?;
    set_executable(&companion_tmp)?;

    std::fs::rename(&companion_tmp, &companion_dest)?;
    std::fs::rename(&launcher_tmp, &launcher_dest)?;

    Ok(launcher_dest)
}

async fn install_npm(
    entry: &IndexerEntry,
    package: &str,
    _progress: &dyn ProgressHandler,
) -> Result<PathBuf> {
    let npm = which::which("npm")
        .context("npm not found on PATH. Install Node.js to use this indexer, or install the indexer manually.")?;

    let dir = install_dir();
    let prefix_dir = dir.join("npm");
    std::fs::create_dir_all(&prefix_dir)?;

    let pkg_spec = format!("{}@{}", package, entry.version);
    tracing::info!(%pkg_spec, "installing via npm");

    let status = tokio::process::Command::new(&npm)
        .args(["install", "--prefix"])
        .arg(&prefix_dir)
        .arg(&pkg_spec)
        .status()
        .await
        .context("Failed to run npm install")?;

    if !status.success() {
        bail!("npm install failed with {}", status);
    }

    apply_npm_compatibility_repairs(entry, &prefix_dir, package)?;

    let bin_dir = prefix_dir.join("node_modules").join(".bin");
    let binary = if cfg!(windows) {
        bin_dir.join(format!("{}.cmd", entry.binary_name))
    } else {
        bin_dir.join(&entry.binary_name)
    };

    if !binary.exists() {
        bail!(
            "npm install succeeded but {} not found at {}",
            entry.binary_name,
            binary.display()
        );
    }

    Ok(binary)
}

fn apply_npm_compatibility_repairs(
    entry: &IndexerEntry,
    prefix_dir: &Path,
    package: &str,
) -> Result<()> {
    if cfg!(windows) && entry.indexer_name == "scip-python" {
        let repaired = repair_scip_python_windows_path_separator_regex(prefix_dir, package)?;
        if repaired {
            tracing::info!(
                indexer = %entry.indexer_name,
                "applied Windows compatibility repair for npm indexer"
            );
        }
    }

    Ok(())
}

fn repair_scip_python_windows_path_separator_regex(
    prefix_dir: &Path,
    package: &str,
) -> Result<bool> {
    let bundle = npm_package_dir(prefix_dir, package)
        .join("dist")
        .join("scip-python.js");
    let source = match std::fs::read_to_string(&bundle) {
        Ok(source) => source,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("Cannot read {}", bundle.display())),
    };

    let broken = r#"new RegExp(o.sep,"g")"#;
    let fixed = r#"new RegExp(o.sep.replace(/[.*+?^${}()|[\]\\]/g,"\\$&"),"g")"#;
    if !source.contains(broken) {
        return Ok(false);
    }

    // scip-python 0.6.6 builds an invalid regex from `path.sep` on Windows
    // (`\` is not a valid regex source by itself). Escape the separator in
    // the installed bundle until the upstream package ships the same fix.
    std::fs::write(&bundle, source.replace(broken, fixed))
        .with_context(|| format!("Cannot write {}", bundle.display()))?;
    Ok(true)
}

async fn install_dotnet_tool(
    entry: &IndexerEntry,
    package: &str,
    _progress: &dyn ProgressHandler,
) -> Result<PathBuf> {
    let dotnet = which::which("dotnet")
        .context("dotnet not found on PATH. Install the .NET SDK to use this indexer, or install the indexer manually.")?;

    let dir = install_dir();
    let tool_dir = dir.join("dotnet-tools");
    std::fs::create_dir_all(&tool_dir)?;

    tracing::info!(package, version = %entry.version, "installing via dotnet tool");

    let status = tokio::process::Command::new(&dotnet)
        .args(["tool", "install", "--tool-path"])
        .arg(&tool_dir)
        .arg(package)
        .args(["--version", &entry.version])
        .status()
        .await
        .context("Failed to run dotnet tool install")?;

    if !status.success() {
        bail!("dotnet tool install failed with {}", status);
    }

    let binary = if cfg!(windows) {
        tool_dir.join(format!("{}.exe", entry.binary_name))
    } else {
        tool_dir.join(&entry.binary_name)
    };

    if !binary.exists() {
        bail!(
            "dotnet tool install succeeded but {} not found at {}",
            entry.binary_name,
            binary.display()
        );
    }

    Ok(binary)
}

// ---------------------------------------------------------------------------
// Main dispatch
// ---------------------------------------------------------------------------

pub(crate) fn repair_existing_indexer(entry: &IndexerEntry) -> Result<()> {
    let dir = install_dir();
    repair_existing_indexer_from(entry, &dir)
}

fn repair_existing_indexer_from(entry: &IndexerEntry, install_root: &Path) -> Result<()> {
    if let InstallMethod::Npm { package } = &entry.install_method {
        apply_npm_compatibility_repairs(entry, &install_root.join("npm"), package)?;
    }

    Ok(())
}

/// Install an indexer using its configured install method.
pub async fn install_indexer_at_version(
    entry: &IndexerEntry,
    version: &str,
    progress: &dyn ProgressHandler,
) -> Result<PathBuf> {
    let mut versioned = entry.clone();
    versioned.version = version.to_owned();
    install_indexer(&versioned, progress).await
}

pub async fn install_indexer(
    entry: &IndexerEntry,
    progress: &dyn ProgressHandler,
) -> Result<PathBuf> {
    match &entry.install_method {
        InstallMethod::GitHubBinary { asset_pattern } => {
            install_github_binary(entry, asset_pattern, progress).await
        }
        InstallMethod::GitHubGz { asset_pattern } => {
            install_github_gz(entry, asset_pattern, progress).await
        }
        InstallMethod::GitHubTarGz {
            asset_pattern,
            binary_path_in_archive,
        } => {
            install_github_tar_gz(
                entry,
                asset_pattern,
                binary_path_in_archive.as_deref(),
                progress,
            )
            .await
        }
        InstallMethod::GitHubZip {
            asset_pattern,
            binary_path_in_archive,
        } => {
            install_github_zip(
                entry,
                asset_pattern,
                binary_path_in_archive.as_deref(),
                progress,
            )
            .await
        }
        InstallMethod::GitHubLauncher {
            unix_asset,
            windows_asset,
        } => install_github_launcher(entry, unix_asset, windows_asset, progress).await,
        InstallMethod::Npm { package } => install_npm(entry, package, progress).await,
        InstallMethod::DotnetTool { package } => {
            install_dotnet_tool(entry, package, progress).await
        }
        InstallMethod::CoveredBy {
            indexer_name,
            reason,
        } => {
            bail!(
                "Indexer '{}' is covered by '{}': {}",
                entry.indexer_name,
                indexer_name,
                reason
            )
        }
        InstallMethod::Unsupported { reason } => {
            bail!(
                "Indexer \'{}\' cannot be automatically installed: {}",
                entry.indexer_name,
                reason
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::backend::BackendCapabilities;
    use std::io::Write;

    fn entry_with_method(binary_name: &str, install_method: InstallMethod) -> IndexerEntry {
        IndexerEntry {
            indexer_name: binary_name.to_string(),
            language: "test".to_string(),
            github_repo: "owner/repo".to_string(),
            binary_name: binary_name.to_string(),
            version: "1.0.0".to_string(),
            default_args: Vec::new(),
            output_file: "index.scip".to_string(),
            install_method,
            backend_capabilities: BackendCapabilities::native(),
        }
    }

    #[test]
    fn test_platform_os_is_known() {
        let os = platform_os();
        assert!(
            ["linux", "darwin", "windows"].contains(&os),
            "unexpected os: {os}"
        );
    }

    #[test]
    fn test_platform_arch_is_known() {
        let arch = platform_arch();
        assert!(
            ["x86_64", "aarch64"].contains(&arch),
            "unexpected arch: {arch}"
        );
    }

    #[test]
    fn test_goreleaser_arch_is_known() {
        let arch = goreleaser_arch();
        assert!(
            ["amd64", "arm64"].contains(&arch),
            "unexpected goreleaser arch: {arch}"
        );
    }

    #[test]
    fn test_target_triple_is_known() {
        let triple = target_triple();
        assert!(
            !triple.starts_with("unknown"),
            "unexpected target triple: {triple}"
        );
    }

    #[test]
    fn test_resolve_pattern_all_placeholders() {
        let pattern = "tool-{version}-{os}-{arch}-{target_triple}-{goreleaser_arch}{ext}";
        let result = resolve_pattern(pattern, "1.2.3");
        assert!(result.contains("1.2.3"));
        assert!(result.contains(platform_os()));
        assert!(result.contains(platform_arch()));
        assert!(result.contains(target_triple()));
        assert!(result.contains(goreleaser_arch()));
        assert!(!result.contains('{'));
    }

    #[test]
    fn test_resolve_pattern_no_placeholders() {
        assert_eq!(resolve_pattern("plain-name", "v1"), "plain-name");
    }

    #[test]
    fn test_github_release_url_format() {
        let url = github_release_url("owner/repo", "v1.0", "binary-linux");
        assert_eq!(
            url,
            "https://github.com/owner/repo/releases/download/v1.0/binary-linux"
        );
    }

    #[test]
    fn chooses_first_release_with_expected_asset() -> Result<()> {
        let entry = entry_with_method(
            "tool",
            InstallMethod::GitHubBinary {
                asset_pattern: "tool-{version}-{os}".to_string(),
            },
        );
        let releases = vec![
            GitHubRelease {
                tag_name: "v2.0.0".to_string(),
                prerelease: false,
                draft: false,
                assets: vec![GitHubAsset {
                    name: "tool-v2.0.0-other".to_string(),
                }],
            },
            GitHubRelease {
                tag_name: "v1.9.0".to_string(),
                prerelease: false,
                draft: false,
                assets: vec![GitHubAsset {
                    name: format!("tool-v1.9.0-{}", platform_os()),
                }],
            },
        ];

        assert_eq!(
            first_compatible_github_release_tag(&entry, &releases)?,
            "v1.9.0"
        );
        Ok(())
    }

    #[test]
    fn matches_release_assets_with_normalized_version() -> Result<()> {
        let entry = entry_with_method(
            "tool",
            InstallMethod::GitHubBinary {
                asset_pattern: "tool_{version}_{os}_{goreleaser_arch}.tar.gz".to_string(),
            },
        );
        let releases = vec![GitHubRelease {
            tag_name: "v1.9.0".to_string(),
            prerelease: false,
            draft: false,
            assets: vec![GitHubAsset {
                name: format!("tool_1.9.0_{}_{}.tar.gz", platform_os(), goreleaser_arch()),
            }],
        }];

        assert_eq!(
            first_compatible_github_release_tag(&entry, &releases)?,
            "v1.9.0"
        );
        Ok(())
    }

    #[test]
    fn skips_prerelease_moving_tags_for_latest_compatible_release() -> Result<()> {
        let entry = entry_with_method(
            "tool",
            InstallMethod::GitHubBinary {
                asset_pattern: "tool-{version}-{os}".to_string(),
            },
        );
        let releases = vec![
            GitHubRelease {
                tag_name: "nightly".to_string(),
                prerelease: true,
                draft: false,
                assets: vec![GitHubAsset {
                    name: format!("tool-nightly-{}", platform_os()),
                }],
            },
            GitHubRelease {
                tag_name: "2026-05-25".to_string(),
                prerelease: false,
                draft: false,
                assets: vec![GitHubAsset {
                    name: format!("tool-2026-05-25-{}", platform_os()),
                }],
            },
        ];

        assert_eq!(
            first_compatible_github_release_tag(&entry, &releases)?,
            "2026-05-25"
        );
        Ok(())
    }

    #[test]
    fn pattern_asset_candidates_include_raw_and_normalized_versions() {
        assert_eq!(
            pattern_asset_candidates("tool_{version}_{os}.tar.gz", "v1.9.0"),
            vec![
                format!("tool_v1.9.0_{}.tar.gz", platform_os()),
                format!("tool_1.9.0_{}.tar.gz", platform_os()),
            ]
        );
    }

    #[test]
    fn linux_asset_resolution_uses_linux_platform_even_on_windows_hosts() -> Result<()> {
        let ruby = entry_with_method(
            "scip-ruby",
            InstallMethod::GitHubBinary {
                asset_pattern: "scip-ruby-{arch}-{os}".to_string(),
            },
        );
        let clang = entry_with_method(
            "scip-clang",
            InstallMethod::GitHubBinary {
                asset_pattern: "scip-clang-{arch}-{os}".to_string(),
            },
        );

        assert_eq!(
            expected_github_assets_for_platform(
                &ruby,
                "scip-ruby-v0.4.7",
                IndexerAssetPlatform::LinuxX86_64
            )?,
            vec!["scip-ruby-x86_64-linux"]
        );
        assert_eq!(
            expected_github_assets_for_platform(
                &clang,
                "v0.4.0",
                IndexerAssetPlatform::LinuxX86_64
            )?,
            vec!["scip-clang-x86_64-linux"]
        );
        Ok(())
    }

    #[test]
    fn test_extract_gz_roundtrip() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let dir = tempfile::tempdir().unwrap();
        let gz_path = dir.path().join("test.gz");
        let out_path = dir.path().join("test.bin");

        // Create a gzipped file
        let data = b"hello world binary content";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(data).unwrap();
        let compressed = encoder.finish().unwrap();
        std::fs::write(&gz_path, &compressed).unwrap();

        // Extract
        extract_gz(&gz_path, &out_path).unwrap();
        let result = std::fs::read(&out_path).unwrap();
        assert_eq!(result, data);
    }

    #[test]
    fn test_extract_tar_gz_roundtrip() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("test.tar.gz");
        let dest_dir = dir.path().join("out");
        std::fs::create_dir_all(&dest_dir).unwrap();

        // Create a tar.gz with a binary inside
        let data = b"fake binary content";
        let gz_file = std::fs::File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(gz_file, Compression::fast());
        let mut tar_builder = tar::Builder::new(encoder);

        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar_builder
            .append_data(&mut header, "my-tool", &data[..])
            .unwrap();
        let encoder = tar_builder.into_inner().unwrap();
        encoder.finish().unwrap();

        // Extract
        let result = extract_tar_gz(&archive_path, &dest_dir, "my-tool", None).unwrap();
        let expected = if cfg!(windows) {
            dest_dir.join("my-tool.exe")
        } else {
            dest_dir.join("my-tool")
        };
        assert_eq!(result, expected);
        assert_eq!(std::fs::read(&result).unwrap(), data);
    }

    #[test]
    fn test_extract_tar_gz_nested_path() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("test.tar.gz");
        let dest_dir = dir.path().join("out");
        std::fs::create_dir_all(&dest_dir).unwrap();

        let data = b"nested binary";
        let gz_file = std::fs::File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(gz_file, Compression::fast());
        let mut tar_builder = tar::Builder::new(encoder);

        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar_builder
            .append_data(&mut header, "subdir/my-tool", &data[..])
            .unwrap();
        let encoder = tar_builder.into_inner().unwrap();
        encoder.finish().unwrap();

        // Extract by filename match
        let result = extract_tar_gz(&archive_path, &dest_dir, "my-tool", None).unwrap();
        assert_eq!(std::fs::read(&result).unwrap(), data);
    }

    #[test]
    #[cfg(windows)]
    fn test_extract_tar_gz_matches_windows_exe_name() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("test.tar.gz");
        let dest_dir = dir.path().join("out");
        std::fs::create_dir_all(&dest_dir).unwrap();

        let data = b"windows binary";
        let gz_file = std::fs::File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(gz_file, Compression::fast());
        let mut tar_builder = tar::Builder::new(encoder);

        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar_builder
            .append_data(&mut header, "my-tool.exe", &data[..])
            .unwrap();
        let encoder = tar_builder.into_inner().unwrap();
        encoder.finish().unwrap();

        let result = extract_tar_gz(&archive_path, &dest_dir, "my-tool", None).unwrap();
        assert_eq!(result, dest_dir.join("my-tool.exe"));
        assert_eq!(std::fs::read(&result).unwrap(), data);
    }

    #[test]
    fn test_extract_zip_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("test.zip");
        let dest_dir = dir.path().join("out");
        std::fs::create_dir_all(&dest_dir).unwrap();

        // Create a zip with a binary inside
        let data = b"zip binary content";
        let zip_file = std::fs::File::create(&archive_path).unwrap();
        let mut writer = zip::ZipWriter::new(zip_file);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("my-tool", options).unwrap();
        writer.write_all(data).unwrap();
        writer.finish().unwrap();

        // Extract
        let result = extract_zip(&archive_path, &dest_dir, "my-tool", None).unwrap();
        assert!(result.exists());
        let content = std::fs::read(&result).unwrap();
        assert_eq!(content, data);
    }

    #[test]
    fn test_extract_tar_gz_missing_binary_errors() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let dir = tempfile::tempdir().unwrap();
        let archive_path = dir.path().join("test.tar.gz");
        let dest_dir = dir.path().join("out");
        std::fs::create_dir_all(&dest_dir).unwrap();

        // Empty tar.gz
        let gz_file = std::fs::File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(gz_file, Compression::fast());
        let tar_builder = tar::Builder::new(encoder);
        let encoder = tar_builder.into_inner().unwrap();
        encoder.finish().unwrap();

        let result = extract_tar_gz(&archive_path, &dest_dir, "missing", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"),);
    }

    #[test]
    fn repairs_scip_python_windows_path_separator_regex() {
        let dir = tempfile::tempdir().unwrap();
        let prefix_dir = dir.path().join("npm");
        let bundle = super::super::npm_package_dir(&prefix_dir, "@sourcegraph/scip-python")
            .join("dist")
            .join("scip-python.js");
        std::fs::create_dir_all(bundle.parent().unwrap()).unwrap();
        std::fs::write(
            &bundle,
            r#"const o={sep:"\\"};const a=new RegExp(o.sep,"g");"#,
        )
        .unwrap();

        let repaired = repair_scip_python_windows_path_separator_regex(
            &prefix_dir,
            "@sourcegraph/scip-python",
        )
        .unwrap();
        let contents = std::fs::read_to_string(&bundle).unwrap();

        assert!(repaired);
        assert!(
            contents.contains(r#"new RegExp(o.sep.replace(/[.*+?^${}()|[\]\\]/g,"\\$&"),"g")"#)
        );
    }

    #[test]
    fn scip_python_regex_repair_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let prefix_dir = dir.path().join("npm");
        let bundle = super::super::npm_package_dir(&prefix_dir, "@sourcegraph/scip-python")
            .join("dist")
            .join("scip-python.js");
        std::fs::create_dir_all(bundle.parent().unwrap()).unwrap();
        std::fs::write(
            &bundle,
            r#"const o={sep:"\\"};const a=new RegExp(o.sep.replace(/[.*+?^${}()|[\]\\]/g,"\\$&"),"g");"#,
        )
        .unwrap();

        let repaired = repair_scip_python_windows_path_separator_regex(
            &prefix_dir,
            "@sourcegraph/scip-python",
        )
        .unwrap();

        assert!(!repaired);
    }

    #[test]
    fn repairs_existing_scip_python_npm_install() {
        let dir = tempfile::tempdir().unwrap();
        let install_root = dir.path().join("bin");
        let prefix_dir = install_root.join("npm");
        let bundle = super::super::npm_package_dir(&prefix_dir, "@sourcegraph/scip-python")
            .join("dist")
            .join("scip-python.js");
        std::fs::create_dir_all(bundle.parent().unwrap()).unwrap();
        std::fs::write(
            &bundle,
            r#"const o={sep:"\\"};const a=new RegExp(o.sep,"g");"#,
        )
        .unwrap();
        let entry = IndexerEntry {
            indexer_name: "scip-python".to_string(),
            language: "python".to_string(),
            github_repo: "sourcegraph/scip-python".to_string(),
            binary_name: "scip-python".to_string(),
            version: "0.6.6".to_string(),
            default_args: Vec::new(),
            output_file: "index.scip".to_string(),
            install_method: InstallMethod::Npm {
                package: "@sourcegraph/scip-python".to_string(),
            },
            backend_capabilities: BackendCapabilities::native(),
        };

        repair_existing_indexer_from(&entry, &install_root).unwrap();

        let contents = std::fs::read_to_string(&bundle).unwrap();
        // Existing npm installs are only patched on Windows, where the
        // scip-python bundle builds an invalid regex from `path.sep`.
        if cfg!(windows) {
            assert!(
                contents.contains(r#"new RegExp(o.sep.replace(/[.*+?^${}()|[\]\\]/g,"\\$&"),"g")"#)
            );
        } else {
            assert!(contents.contains(r#"new RegExp(o.sep,"g")"#));
        }
    }
}
