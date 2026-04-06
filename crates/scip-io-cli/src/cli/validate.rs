use anyhow::Result;
use console::style;

use scip_io_core::validate::validate_scip_file;

use super::ValidateArgs;

pub async fn run(args: ValidateArgs) -> Result<()> {
    let result = validate_scip_file(&args.input)?;

    match args.format.as_deref() {
        Some("json") => {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        _ => {
            if result.valid {
                println!(
                    "{} Valid SCIP index: {}",
                    style("v").green().bold(),
                    args.input.display()
                );
            } else {
                println!(
                    "{} Invalid SCIP index: {}",
                    style("x").red().bold(),
                    args.input.display()
                );
            }

            for err in &result.errors {
                println!("  Error [{}]: {}", err.kind, err.message);
            }
            for warn in &result.warnings {
                println!("  Warning: {}", warn);
            }

            if let Some(stats) = &result.stats {
                println!("  Documents:   {}", stats.documents);
                println!("  Symbols:     {}", stats.symbols);
                println!("  Occurrences: {}", stats.occurrences);
                if !stats.languages.is_empty() {
                    println!("  Languages:   {}", stats.languages.join(", "));
                }
            }
        }
    }

    Ok(())
}
