use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use foci_tune::assets;
use foci_tune::server::{ServeConfig, serve};
use foci_tune::verify::{Kind, verify_archive};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Moonraker base URL.
    #[arg(long, default_value = "http://127.0.0.1:7125")]
    moonraker_url: String,

    /// Moonraker API key.
    #[arg(long)]
    api_key: Option<String>,

    /// Address the web UI and API listen on.
    #[arg(long, default_value = "0.0.0.0:7130")]
    listen: String,

    /// Validate the build and exit without touching the network or USB.
    #[arg(long)]
    check: bool,

    /// Serve against a simulated printer and board instead of Moonraker and USB.
    #[cfg(debug_assertions)]
    #[arg(long)]
    fake: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Compare analysis output against an archived capture set.
    VerifyArchive {
        /// Analysis to run and compare.
        #[arg(long, value_enum)]
        kind: Kind,
        /// Capture files, plain or zstd-compressed.
        #[arg(required = true)]
        captures: Vec<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.check {
        let index = assets::index_html();
        let ok = assets::check_report(
            index.as_ref().map(|a| &*a.data),
            &assets::list(),
            &mut std::io::stdout().lock(),
        );
        return if matches!(ok, Ok(true)) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    match cli.command {
        Some(Command::VerifyArchive { kind, captures }) => {
            match verify_archive(kind, &captures, &mut std::io::stdout().lock()) {
                Ok(summary) if summary.ok() => ExitCode::SUCCESS,
                Ok(_) => ExitCode::FAILURE,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        None => {
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::INFO)
                .init();
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: cannot start the async runtime: {e}");
                    return ExitCode::FAILURE;
                }
            };
            #[cfg(debug_assertions)]
            if cli.fake {
                return match runtime.block_on(foci_tune::devfake::serve_fake(cli.listen)) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("error: {e}");
                        ExitCode::FAILURE
                    }
                };
            }
            let config = ServeConfig {
                moonraker_url: cli.moonraker_url,
                api_key: cli.api_key,
                listen: cli.listen,
            };
            match runtime.block_on(serve(config)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}
