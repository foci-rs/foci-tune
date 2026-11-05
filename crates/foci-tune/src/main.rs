use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use foci_tune::assets;
use foci_tune::server::{ServeConfig, serve};
use foci_tune::setup::{
    self, Host, InstallPlan, Runner, Setup, SetupError, SystemRunner, UnexecutedRunner,
};
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
    /// Install the udev rule and systemd unit and start the service (Linux, root).
    Setup {
        /// User the service runs as; added to the plugdev group.
        #[arg(long, default_value = "pi", conflicts_with = "remove")]
        user: String,
        /// Print what would be done, including the file contents, and change nothing.
        #[arg(long)]
        dry_run: bool,
        /// Stop and disable the service and remove the installed files.
        #[arg(long)]
        remove: bool,
        /// Place every system path under this directory and run no external commands.
        #[arg(long, hide = true)]
        root: Option<PathBuf>,
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
        Some(Command::Setup {
            user,
            dry_run,
            remove,
            root,
        }) => match run_setup(&user, dry_run, remove, root) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
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

fn run_setup(
    user: &str,
    dry_run: bool,
    remove: bool,
    root: Option<PathBuf>,
) -> Result<(), SetupError> {
    let mut runner: Box<dyn Runner> = if root.is_some() {
        Box::new(UnexecutedRunner)
    } else {
        Box::new(SystemRunner)
    };
    let config = Setup {
        prefix: root,
        dry_run,
        host: Host::current(),
    };
    let mut out = std::io::stdout().lock();
    if remove {
        return setup::remove(&config, runner.as_mut(), &mut out);
    }
    let exe = std::env::current_exe().map_err(|source| SetupError::Io {
        path: PathBuf::from("the running executable"),
        source,
    })?;
    let plan = InstallPlan { user, exe: &exe };
    setup::install(&config, &plan, runner.as_mut(), &mut out)
}
