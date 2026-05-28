use anyhow::Result;
use console::style;

use scip_io_core::detect::{LanguageScanOptions, scan_languages_with_options};

use super::DetectArgs;

pub async fn run(args: DetectArgs) -> Result<()> {
    let path = args
        .path
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let path = path.canonicalize()?;

    tracing::info!(?path, "scanning for languages");

    let languages = scan_languages_with_options(
        &path,
        LanguageScanOptions {
            max_depth: args.depth,
            ..Default::default()
        },
    )?;

    if languages.is_empty() {
        match args.format.as_str() {
            "json" => {
                println!("[]");
            }
            _ => {
                println!(
                    "{} No supported languages detected in {}",
                    style("!").yellow(),
                    path.display()
                );
            }
        }
        return Ok(());
    }

    match args.format.as_str() {
        "json" => {
            let json = serde_json::to_string_pretty(&languages)?;
            println!("{}", json);
        }
        _ => {
            let names: Vec<&str> = languages.iter().map(|l| l.name()).collect();
            println!(
                "{} Detected: {}",
                style("v").green().bold(),
                names.join(", ")
            );

            for lang in &languages {
                let readiness = if lang.indexer_ready {
                    "ready"
                } else {
                    "needs setup"
                };
                println!(
                    "  {} {} (found {}; {}; {})",
                    style("*").dim(),
                    lang.name(),
                    lang.evidence(),
                    lang.evidence_kind,
                    readiness
                );
                if let Some(message) = &lang.readiness_message {
                    println!("      {} {}", style("!").yellow(), message);
                }
            }
        }
    }

    Ok(())
}
