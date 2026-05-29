use anyhow::{Result, bail};
use protobuf::Message;
use scip::types::Index;
use scip_io_core::LanguageKind;
use scip_io_core::indexer::backend::{
    BackendPreference, ExecutionBackendKind, probe_docker, probe_wsl, wsl_path_for_windows_path,
};
use scip_io_core::indexer::registry::REGISTRY;
use scip_io_core::indexer::runner;
use scip_io_core::validate::validate_scip_file;

#[tokio::test]
#[ignore = "requires Windows plus a configured WSL distro with Ruby project dependencies"]
async fn wsl_ruby_backend_generates_valid_scip() -> Result<()> {
    if !cfg!(windows) {
        eprintln!("skipping: WSL backend smoke is only meaningful on Windows");
        return Ok(());
    }
    let probe = probe_wsl().await;
    if !probe.available {
        eprintln!("skipping: WSL unavailable: {:?}", probe.detail);
        return Ok(());
    }

    let dir = tempfile::tempdir()?;
    std::fs::write(
        dir.path().join("app.rb"),
        "class App\n  def call\n    1\n  end\nend\n",
    )?;

    let lang = LanguageKind::Ruby.with_evidence("app.rb".into());
    let entry = REGISTRY
        .runnable_for(&lang)
        .ok_or_else(|| anyhow::anyhow!("missing ruby indexer"))?;
    let output = runner::run_indexer_with_configs_and_backend(
        None,
        entry,
        dir.path(),
        &lang,
        &[],
        BackendPreference {
            kind: ExecutionBackendKind::Wsl,
            docker_image: None,
            wsl_distro: None,
        },
    )
    .await?;

    let validation = validate_scip_file(&output)?;
    if !validation.valid {
        bail!("invalid ruby SCIP: {:?}", validation.errors);
    }
    if !validation.warnings.is_empty() {
        bail!("ruby SCIP warnings: {:?}", validation.warnings);
    }
    assert_document_paths(&output, &["app.rb"])?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Windows plus a configured WSL distro compatible with scip-clang"]
async fn wsl_clang_backend_generates_valid_scip() -> Result<()> {
    if !cfg!(windows) {
        eprintln!("skipping: WSL backend smoke is only meaningful on Windows");
        return Ok(());
    }
    let probe = probe_wsl().await;
    if !probe.available {
        eprintln!("skipping: WSL unavailable: {:?}", probe.detail);
        return Ok(());
    }

    let dir = tempfile::tempdir()?;
    std::fs::create_dir_all(dir.path().join("src"))?;
    std::fs::write(
        dir.path().join("src").join("main.cc"),
        "int main() { return 0; }\n",
    )?;
    let linux_root = wsl_path_for_windows_path(dir.path()).await?;
    std::fs::write(
        dir.path().join("compile_commands.json"),
        format!(
            r#"[{{"directory":"{}","command":"clang++ -c src/main.cc","file":"src/main.cc"}}]"#,
            linux_root.replace('\\', "\\\\").replace('"', "\\\"")
        ),
    )?;

    let lang = LanguageKind::Cpp.with_evidence("compile_commands.json".into());
    let entry = REGISTRY
        .runnable_for(&lang)
        .ok_or_else(|| anyhow::anyhow!("missing cpp indexer"))?;
    let output = runner::run_indexer_with_configs_and_backend(
        None,
        entry,
        dir.path(),
        &lang,
        &[],
        BackendPreference {
            kind: ExecutionBackendKind::Wsl,
            docker_image: None,
            wsl_distro: None,
        },
    )
    .await?;

    let validation = validate_scip_file(&output)?;
    if !validation.valid {
        bail!("invalid clang SCIP: {:?}", validation.errors);
    }
    if !validation.warnings.is_empty() {
        bail!("clang SCIP warnings: {:?}", validation.warnings);
    }
    assert_document_paths(&output, &["src/main.cc"])?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires Docker plus a Linux image/toolchain compatible with scip-clang"]
async fn docker_clang_backend_generates_valid_scip() -> Result<()> {
    if !cfg!(windows) {
        eprintln!("skipping: Docker backend smoke is only meaningful on Windows");
        return Ok(());
    }
    let probe = probe_docker().await;
    if !probe.available {
        eprintln!("skipping: Docker unavailable: {:?}", probe.detail);
        return Ok(());
    }

    let dir = tempfile::tempdir()?;
    std::fs::create_dir_all(dir.path().join("src"))?;
    std::fs::write(
        dir.path().join("src").join("main.cc"),
        "int main() { return 0; }\n",
    )?;
    std::fs::write(
        dir.path().join("compile_commands.json"),
        r#"[{"directory":"/workspace","command":"clang++ -c src/main.cc","file":"src/main.cc"}]"#,
    )?;

    let lang = LanguageKind::Cpp.with_evidence("compile_commands.json".into());
    let entry = REGISTRY
        .runnable_for(&lang)
        .ok_or_else(|| anyhow::anyhow!("missing cpp indexer"))?;
    let output = runner::run_indexer_with_configs_and_backend(
        None,
        entry,
        dir.path(),
        &lang,
        &[],
        BackendPreference {
            kind: ExecutionBackendKind::Docker,
            docker_image: None,
            wsl_distro: None,
        },
    )
    .await?;

    let validation = validate_scip_file(&output)?;
    if !validation.valid {
        bail!("invalid clang SCIP: {:?}", validation.errors);
    }
    if !validation.warnings.is_empty() {
        bail!("clang SCIP warnings: {:?}", validation.warnings);
    }
    assert_document_paths(&output, &["src/main.cc"])?;
    Ok(())
}

fn assert_document_paths(output: &std::path::Path, expected: &[&str]) -> Result<()> {
    let index = Index::parse_from_bytes(&std::fs::read(output)?)?;
    let paths = index
        .documents
        .iter()
        .map(|document| document.relative_path.as_str())
        .collect::<Vec<_>>();
    if paths != expected {
        bail!(
            "unexpected document paths in {}: {:?}",
            output.display(),
            paths
        );
    }
    Ok(())
}
