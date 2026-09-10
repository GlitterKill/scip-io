//! Pinned scip-java distribution with bundled Kotlin 2.3.21 compiler compatibility.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use super::{IndexerEntry, install};
use crate::progress::ProgressHandler;

pub const REPOSITORY: &str = "GlitterKill/scip-io";
pub const VERSION: &str = "scip-java-v0.12.3-scip-io.2";
pub const PAYLOAD_SHA256: &str = "6a348ada3570002344305e3cf20bc61c94e88a2f6220c298ef81b3980ab1f662";
pub const LAUNCHER_SHA256: &str =
    "843319e0a3c57e588a0dd25edd2fee4621ec9cd152741d3128a5f5366264a593";

pub fn applies(entry: &IndexerEntry) -> bool {
    entry.indexer_name == "scip-java" && entry.github_repo == REPOSITORY
}

pub fn directory(root: &Path) -> PathBuf {
    root.join(VERSION)
}

pub fn installed_path(root: &Path) -> Option<PathBuf> {
    let dir = directory(root);
    let payload = dir.join("scip-java");
    let launcher = dir.join("scip-java.bat");
    (payload.is_file() && (!cfg!(windows) || launcher.is_file())).then_some(if cfg!(windows) {
        launcher
    } else {
        payload
    })
}

pub fn require_version(version: &str) -> Result<()> {
    if version != VERSION {
        bail!("The managed scip-java repair is pinned to {VERSION}, requested {version}");
    }
    Ok(())
}

fn verify_file(path: &Path, expected: &str) -> Result<()> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("Cannot open repaired artifact {}", path.display()))?;
    let mut digest = Sha256::new();
    std::io::copy(&mut file, &mut digest)?;
    if hex::encode(digest.finalize()) != expected {
        bail!("SHA-256 mismatch for repaired artifact {}", path.display());
    }
    Ok(())
}

fn verify_directory(dir: &Path, windows: bool) -> Result<PathBuf> {
    let payload = dir.join("scip-java");
    verify_file(&payload, PAYLOAD_SHA256)?;
    if windows {
        let launcher = dir.join("scip-java.bat");
        verify_file(&launcher, LAUNCHER_SHA256)?;
        Ok(launcher)
    } else {
        Ok(payload)
    }
}

pub async fn install_in(
    root: &Path,
    windows: bool,
    progress: &dyn ProgressHandler,
) -> Result<PathBuf> {
    let base_url = format!("https://github.com/{REPOSITORY}/releases/download/{VERSION}");
    install_from(root, windows, progress, &base_url).await
}

async fn install_from(
    root: &Path,
    windows: bool,
    progress: &dyn ProgressHandler,
    base_url: &str,
) -> Result<PathBuf> {
    let dest = directory(root);
    if dest.exists() {
        // Never trust a version label alone, or execute an incomplete cached pair.
        return verify_directory(&dest, windows);
    }
    std::fs::create_dir_all(root)?;
    let staging = tempfile::tempdir_in(root)?;
    for (name, checksum) in [
        ("scip-java", PAYLOAD_SHA256),
        ("scip-java.bat", LAUNCHER_SHA256),
    ] {
        if name.ends_with(".bat") && !windows {
            continue;
        }
        let file = staging.path().join(name);
        let url = format!("{base_url}/{name}");
        install::download_to_file(&url, &file, "scip-java", progress).await?;
        verify_file(&file, checksum)?;
        install::set_executable(&file)?;
    }
    // Publish the complete verified pair together. Old unversioned installs stay intact.
    if let Err(error) = std::fs::rename(staging.path(), &dest)
        && !dest.exists()
    {
        return Err(error.into());
    }
    // Another installer may have won the race; accept only the same verified bytes.
    verify_directory(&dest, windows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::NoopHandler;

    #[tokio::test]
    async fn resolution_stays_pinned_for_native_and_linux_backends() {
        let entry = super::super::registry::REGISTRY
            .all()
            .iter()
            .find(|entry| entry.language == "java")
            .unwrap();
        assert_eq!(
            install::resolve_latest_compatible_version(entry)
                .await
                .unwrap(),
            VERSION
        );
        assert_eq!(
            install::resolve_latest_compatible_version_for_platform(
                entry,
                install::IndexerAssetPlatform::LinuxX86_64
            )
            .await
            .unwrap(),
            VERSION
        );
        assert!(require_version("v0.12.3").is_err());
    }

    #[tokio::test]
    #[ignore = "requires staged release files in SCIP_JAVA_REPAIR_TEST_ASSETS"]
    async fn real_artifact_download_install_and_cache_smoke() {
        use std::io::{Read, Write};
        let assets = PathBuf::from(std::env::var_os("SCIP_JAVA_REPAIR_TEST_ASSETS").unwrap());
        verify_directory(&assets, true).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for name in ["scip-java", "scip-java.bat"] {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(30)))
                    .unwrap();
                let mut request = [0; 4096];
                let read = socket.read(&mut request).unwrap();
                assert!(
                    String::from_utf8_lossy(&request[..read]).starts_with(&format!("GET /{name} "))
                );
                let mut file = std::fs::File::open(assets.join(name)).unwrap();
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    file.metadata().unwrap().len()
                )
                .unwrap();
                std::io::copy(&mut file, &mut socket).unwrap();
            }
        });
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("scip-java");
        std::fs::write(&legacy, "old cache").unwrap();
        let previous = root.path().join("scip-java-v0.12.3-scip-io.1");
        std::fs::create_dir(&previous).unwrap();
        std::fs::write(previous.join("scip-java"), "previous pinned release").unwrap();
        let path = install_from(root.path(), true, &NoopHandler, &base_url)
            .await
            .unwrap();
        server.join().unwrap();
        assert_eq!(path, directory(root.path()).join("scip-java.bat"));
        assert_eq!(std::fs::read_to_string(legacy).unwrap(), "old cache");
        assert_eq!(
            std::fs::read_to_string(previous.join("scip-java")).unwrap(),
            "previous pinned release"
        );
        // Server is gone: the second install must verify and reuse the local pair.
        assert_eq!(
            install_from(root.path(), true, &NoopHandler, &base_url)
                .await
                .unwrap(),
            path
        );
        std::fs::write(&path, "tampered batch launcher").unwrap();
        assert!(
            install_from(root.path(), true, &NoopHandler, &base_url)
                .await
                .unwrap_err()
                .to_string()
                .contains("SHA-256 mismatch")
        );
    }

    #[tokio::test]
    async fn damaged_versioned_cache_fails_without_touching_legacy_install() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("scip-java");
        std::fs::write(&legacy, "previous install").unwrap();
        let dir = directory(root.path());
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("scip-java"), "truncated download").unwrap();
        let error = install_in(root.path(), true, &NoopHandler)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("SHA-256 mismatch"));
        assert_eq!(std::fs::read_to_string(legacy).unwrap(), "previous install");
        assert!(installed_path(root.path()).is_none() || !cfg!(windows));
    }

    #[test]
    fn checksum_verification_accepts_only_matching_bytes() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), b"abc").unwrap();
        verify_file(
            file.path(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        )
        .unwrap();
        assert!(verify_file(file.path(), PAYLOAD_SHA256).is_err());
    }

    #[tokio::test]
    async fn rejected_download_never_publishes_partial_install() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).unwrap() > 0);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad")
                .unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let error = install_from(root.path(), true, &NoopHandler, &base_url)
            .await
            .unwrap_err();
        server.join().unwrap();
        assert!(error.to_string().contains("SHA-256 mismatch"));
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
