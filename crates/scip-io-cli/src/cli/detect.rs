use anyhow::Result;
use console::style;

use scip_io_core::detect::scan_languages;

use super::DetectArgs;

pub async fn run(args: DetectArgs) -> Result<()> {
    let path = args
        .path
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let path = path.canonicalize()?;

    tracing::info!(?path, "scanning for languages");

    let languages = scan_languages(&path)?;

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
                println!(
                    "  {} {} (found {})",
                    style("*").dim(),
                    lang.name(),
                    lang.evidence()
                );
            }
        }
    }

    Ok(())
}
