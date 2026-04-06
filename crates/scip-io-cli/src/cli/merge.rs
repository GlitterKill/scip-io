use anyhow::Result;
use console::style;

use scip_io_core::merge::merge_scip_files;
use scip_io_core::validate::validate_scip_file;

use super::MergeArgs;

pub async fn run(args: MergeArgs) -> Result<()> {
    println!(
        "{} Merging {} SCIP index files...",
        style(">").cyan().bold(),
        args.inputs.len()
    );

    merge_scip_files(&args.inputs, &args.output)?;

    println!(
        "{} Merged index written to {}",
        style("v").green().bold(),
        args.output.display()
    );

    if args.validate {
        println!(
            "\n{} Validating merged output...",
            style(">").cyan().bold(),
        );
        let result = validate_scip_file(&args.output)?;
        if result.valid {
            println!(
                "{} Valid SCIP index: {}",
                style("v").green().bold(),
                args.output.display()
            );
        } else {
            println!(
                "{} Invalid SCIP index: {}",
                style("x").red().bold(),
                args.output.display()
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

    Ok(())
}
