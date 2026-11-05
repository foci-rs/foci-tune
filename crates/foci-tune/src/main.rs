use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use foci_tune::assets;
use foci_tune::moonraker::{ClientConfig, MoonrakerClient};
use foci_tune::server::{ServeConfig, serve};
use foci_tune::setup::{
    self, Host, InstallPlan, Runner, Setup, SetupError, SystemRunner, UnexecutedRunner,
    asvc_beside_logs,
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
    Setup(SetupArgs),
}

#[derive(clap::Args)]
struct SetupArgs {
    /// User the service runs as; added to the plugdev group. Defaults to `SUDO_USER`.
    #[arg(long, conflicts_with = "remove")]
    user: Option<String>,
    /// Print what would be done, including the file contents, and change nothing.
    #[arg(long)]
    dry_run: bool,
    /// Stop and disable the service and remove the installed files.
    #[arg(long)]
    remove: bool,
    /// Place every system path under this directory and run no external commands.
    #[arg(long, hide = true)]
    root: Option<PathBuf>,
    /// Path of Moonraker's `moonraker.asvc`; looked up through Moonraker when omitted.
    #[arg(long)]
    asvc_path: Option<PathBuf>,
    /// Moonraker base URL used to locate `moonraker.asvc`.
    #[arg(long, default_value = "http://127.0.0.1:7125")]
    moonraker_url: String,
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
        Some(Command::Setup(args)) => match run_setup(args) {
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

fn run_setup(args: SetupArgs) -> Result<(), SetupError> {
    let mut runner: Box<dyn Runner> = if args.root.is_some() {
        Box::new(UnexecutedRunner)
    } else {
        Box::new(SystemRunner)
    };
    let config = Setup {
        prefix: args.root,
        dry_run: args.dry_run,
        host: Host::current(),
    };
    let mut out = std::io::stdout().lock();
    if args.remove {
        let asvc = match resolve_asvc(args.asvc_path, &args.moonraker_url) {
            Ok(path) => Some(path),
            Err(e) => {
                eprintln!("warning: {e}");
                None
            }
        };
        return setup::remove(&config, asvc.as_deref(), runner.as_mut(), &mut out);
    }
    let exe = std::env::current_exe().map_err(|source| SetupError::Io {
        path: PathBuf::from("the running executable"),
        source,
    })?;
    let sudo_user = std::env::var("SUDO_USER").ok();
    let user = setup::resolve_user(
        args.user.as_deref(),
        sudo_user.as_deref().filter(|u| !u.is_empty()),
    )?;
    let asvc = resolve_asvc(args.asvc_path, &args.moonraker_url)?;
    let plan = InstallPlan {
        user: &user,
        exe: &exe,
        asvc: &asvc,
    };
    setup::install(&config, &plan, runner.as_mut(), &mut out)
}

/// `--asvc-path` when given, else the path found through Moonraker. A runtime is built only
/// for the lookup.
fn resolve_asvc(explicit: Option<PathBuf>, moonraker_url: &str) -> Result<PathBuf, SetupError> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| SetupError::Asvc {
            path: PathBuf::from("moonraker.asvc"),
            reason: format!("cannot start the async runtime: {e}"),
        })?;
    runtime.block_on(locate_asvc(moonraker_url))
}

/// Asks Moonraker where its `logs` root is and returns the `moonraker.asvc` beside it.
async fn locate_asvc(moonraker_url: &str) -> Result<PathBuf, SetupError> {
    let roots_url = format!("{}/server/files/roots", moonraker_url.trim_end_matches('/'));
    let fail = |reason: String| SetupError::Asvc {
        path: PathBuf::from(&roots_url),
        reason,
    };
    let client = MoonrakerClient::connect(ClientConfig {
        url: moonraker_url.to_owned(),
        api_key: None,
        backoff_initial: Duration::from_secs(1),
        backoff_max: Duration::from_secs(1),
    });
    let logs = client.logs_root().await.map_err(|e| {
        fail(format!(
            "cannot find Moonraker's logs root: {e}; pass --asvc-path"
        ))
    })?;
    asvc_beside_logs(&logs)
        .ok_or_else(|| fail(format!("{} has no parent directory", logs.display())))
}
