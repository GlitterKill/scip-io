use clap::Parser;

mod cli;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli_args = cli::Cli::parse();
    let rt = tokio::runtime::Runtime::new().unwrap_or_else(|e| {
        eprintln!("Error: failed to create async runtime: {}", e);
        std::process::exit(2);
    });

    let result = match cli_args.command {
        Some(cli::Command::Detect(args)) => rt.block_on(cli::detect::run(args)),
        Some(cli::Command::Index(args)) => rt.block_on(cli::index::run(args)),
        Some(cli::Command::Status(args)) => rt.block_on(cli::status::run(args)),
        Some(cli::Command::Merge(args)) => rt.block_on(cli::merge::run(args)),
        Some(cli::Command::Clean(args)) => rt.block_on(cli::clean::run(args)),
        Some(cli::Command::Validate(args)) => rt.block_on(cli::validate::run(args)),
        Some(cli::Command::UpdateRegistry(args)) => rt.block_on(cli::update_registry::run(args)),
        Some(cli::Command::Gui(_args)) => {
            println!("GUI not yet implemented. Use CLI commands instead.");
            Ok(())
        }
        None => {
            if cli_args.no_gui || std::env::var("SCIP_IO_NO_GUI").is_ok() {
                use clap::CommandFactory;
                cli::Cli::command().print_help().ok();
                println!();
                Ok(())
            } else {
                println!("GUI not yet implemented. Run with a subcommand or --help.");
                Ok(())
            }
        }
    };

    match result {
        Ok(()) => {}
        Err(e) => {
            let msg = format!("{:#}", e);
            // Partial failure from the index command gets exit code 1
            if msg.starts_with("partial-failure:") {
                std::process::exit(1);
            }
            eprintln!("Error: {}", msg);
            std::process::exit(2);
        }
    }
}
