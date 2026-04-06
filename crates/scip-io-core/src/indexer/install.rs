use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;

use crate::indexer::{IndexerEntry, InstallMethod, install_dir};
use crate::progress::{ProgressEvent, ProgressHandler};

// ---------------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------------

fn platform_os() -> &'static str {
    if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}

fn platform_ext() -> &'static str {
    if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    }
}

fn platform_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "unknown"
    }
}

/// Goreleaser-style arch names (used by scip-go).
fn goreleaser_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "amd64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "unknown"
    }
}

/// Rust target triple for the current platform.
fn target_triple() -> &'static str {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        "aarch64-pc-windows-msvc"
    } else {
        "unknown-unknown-unknown"
    }
}

/// Resolve placeholders in an asset pattern string.
fn resolve_pattern(pattern: &str, version: &str) -> String {
    pattern
        .replace("{version}", version)
        .replace("{os}", platform_os())
        .replace("{arch}", platform_arch())
        .replace("{target_triple}", target_triple())
        .replace("{goreleaser_arch}", goreleaser_arch())
        .replace("{ext}", platform_ext())
}

/// Build the full GitHub release download URL.
fn github_release_url(github_repo: &str, version: &str, asset: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/{}/{}",
        github_repo, version, asset,
    )
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

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?;
        let path_str = path.to_string_lossy();

        // Match either the exact path or just the filename component
        let matches = path_str == target || path.file_name().is_some_and(|f| f == binary_name);

        if matches {
            let dest = dest_dir.join(binary_name);
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
    let asset = resolve_pattern(pattern, &entry.version);
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

async fn install_github_gz(
    entry: &IndexerEntry,
    pattern: &str,
    progress: &dyn ProgressHandler,
) -> Result<PathBuf> {
    let asset = resolve_pattern(pattern, &entry.version);
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
    let asset = resolve_pattern(pattern, &entry.version);
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
    let asset = resolve_pattern(pattern, &entry.version);
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
    let asset = if cfg!(windows) {
        resolve_pattern(windows_asset, &entry.version)
    } else {
        resolve_pattern(unix_asset, &entry.version)
    };
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

/// Install an indexer using its configured install method.
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
    use std::io::Write;

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
        assert_eq!(result, dest_dir.join("my-tool"));
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
}
