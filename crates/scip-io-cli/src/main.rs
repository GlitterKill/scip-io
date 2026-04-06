use clap::Parser;

mod cli;

/// Print help output and a short banner pointing at the GUI download.
fn print_help_banner() {
    use clap::CommandFactory;
    println!("SCIP-IO v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("This is the command-line interface. For the graphical desktop app, download");
    println!("the latest installer from:");
    println!("  https://github.com/GlitterKill/scip-io/releases/latest");
    println!("    - Windows: SCIP-IO_<version>_x64-setup.exe or .msi");
    println!("    - macOS:   SCIP-IO_<version>_x64.dmg (Intel) / _aarch64.dmg (Apple Silicon)");
    println!("    - Linux:   SCIP-IO_<version>_amd64.deb or .AppImage");
    println!();
    cli::Cli::command().print_help().ok();
    println!();
}

/// On Windows, if we appear to have been launched by double-clicking the
/// `.exe` from Explorer (i.e., we are the only process attached to the
/// console), wait for the user to press Enter so they can read any output
/// before the console window closes.
#[cfg(windows)]
fn pause_if_launched_from_explorer() {
    // `GetConsoleProcessList` returns the number of processes attached to
    // the current console. If it is 1, we are the only process and the
    // console will disappear as soon as we exit.
    unsafe extern "system" {
        fn GetConsoleProcessList(lpdwProcessList: *mut u32, dwProcessCount: u32) -> u32;
    }
    let mut list = [0u32; 2];
    let count = unsafe { GetConsoleProcessList(list.as_mut_ptr(), 2) };
    if count <= 1 {
        println!("Press Enter to exit...");
        let mut buf = String::new();
        let _ = std::io::stdin().read_line(&mut buf);
    }
}

#[cfg(not(windows))]
fn pause_if_launched_from_explorer() {}

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
            println!("The SCIP-IO graphical app is a separate download.");
            println!("Get it from: https://github.com/GlitterKill/scip-io/releases/latest");
            println!("  - Windows: SCIP-IO_<version>_x64-setup.exe or .msi");
            println!("  - macOS:   SCIP-IO_<version>_x64.dmg / _aarch64.dmg");
            println!("  - Linux:   SCIP-IO_<version>_amd64.deb or .AppImage");
            pause_if_launched_from_explorer();
            Ok(())
        }
        None => {
            print_help_banner();
            pause_if_launched_from_explorer();
            Ok(())
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
