//! `setup`: installs the udev rule and the systemd unit on the Pi, or removes them.

mod files;

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

const LAUNCHER_NAME: &str = "foci-tune";
const RULE_PATH: &str = "etc/udev/rules.d/99-foci-trace.rules";
const UNIT_PATH: &str = "etc/systemd/system/foci-tune.service";
const DROP_IN_PATH: &str = "etc/systemd/system/foci-tune.service.d";
const ENABLED_LINK_PATH: &str = "etc/systemd/system/multi-user.target.wants/foci-tune.service";

/// The identity and platform the command runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host {
    /// Effective user id.
    pub euid: u32,
    /// Whether this is a Linux host.
    pub linux: bool,
}

impl Host {
    /// The host this process runs on.
    pub fn current() -> Self {
        // SAFETY: geteuid has no preconditions, touches no memory and always succeeds.
        let euid = unsafe { libc::geteuid() };
        Self {
            euid,
            linux: cfg!(target_os = "linux"),
        }
    }
}

/// Options shared by install and remove.
#[derive(Debug, Clone)]
pub struct Setup {
    /// Directory every system path is placed under. When set, external commands are not
    /// executed and neither root nor Linux is required.
    pub prefix: Option<PathBuf>,
    /// Print the plan and change nothing.
    pub dry_run: bool,
    /// Identity and platform checked before a real run.
    pub host: Host,
}

impl Setup {
    fn path(&self, rel: &str) -> PathBuf {
        self.prefix.as_deref().unwrap_or(Path::new("/")).join(rel)
    }

    fn check_host(&self, sudo_hint: &'static str) -> Result<(), SetupError> {
        if self.prefix.is_some() {
            return Ok(());
        }
        if !self.host.linux {
            return Err(SetupError::NotLinux);
        }
        if !self.dry_run && self.host.euid != 0 {
            return Err(SetupError::NotRoot(sudo_hint));
        }
        Ok(())
    }
}

/// What a [`Runner`] did with a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandOutcome {
    /// The command ran and succeeded.
    Ran,
    /// The command was deliberately not executed.
    NotExecuted,
}

/// Executes the external commands setup needs (`usermod`, `udevadm`, `systemctl`).
pub trait Runner {
    /// Runs `argv` (program first).
    ///
    /// # Errors
    ///
    /// A description of the failure when the command cannot start or exits non-zero.
    fn run(&mut self, argv: &[&str]) -> Result<CommandOutcome, String>;
}

/// Runs commands on this system.
#[derive(Debug, Default)]
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&mut self, argv: &[&str]) -> Result<CommandOutcome, String> {
        let output = Command::new(argv[0])
            .args(&argv[1..])
            .output()
            .map_err(|e| format!("cannot run {}: {e}", argv[0]))?;
        if output.status.success() {
            Ok(CommandOutcome::Ran)
        } else {
            Err(format!(
                "{}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

/// Runs nothing; used with a path prefix, where the commands would act on the real system.
#[derive(Debug, Default)]
pub struct UnexecutedRunner;

impl Runner for UnexecutedRunner {
    fn run(&mut self, _argv: &[&str]) -> Result<CommandOutcome, String> {
        Ok(CommandOutcome::NotExecuted)
    }
}

/// Errors from `setup`.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    /// Setup only supports Linux hosts.
    #[error("foci-tune setup only runs on Linux (systemd and udev)")]
    NotLinux,
    /// A real run needs root; carries the command to retry with.
    #[error("foci-tune setup changes system files and must run as root; run: {0}")]
    NotRoot(&'static str),
    /// The service user name is not a plain account name.
    #[error("invalid user name {0:?}")]
    InvalidUser(String),
    /// Neither `--user` nor `SUDO_USER` named the service user.
    #[error("cannot tell which user the service should run as; run with sudo or pass --user")]
    NoUser,
    /// The service must not run as root.
    #[error("the service must not run as root; run with sudo from the service user or pass --user")]
    RootUser,
    /// The service user is not in the passwd database.
    #[error("user {0} does not exist")]
    UnknownUser(String),
    /// The install directory path cannot be written into a systemd unit unescaped.
    #[error("install directory {} is not usable: {reason}", path.display())]
    BadInstallDir {
        /// The rejected directory.
        path: PathBuf,
        /// Why it was rejected.
        reason: &'static str,
    },
    /// The launcher symlink beside the running binary is absent or points elsewhere.
    #[error(
        "{} must be a symlink to the running binary; run setup through the installer",
        .0.display()
    )]
    MissingLauncher(PathBuf),
    /// The group the udev rule grants access to does not exist.
    #[error(
        "group {0} does not exist; setup does not create groups, create it \
         (sudo groupadd {0}) and run setup again"
    )]
    MissingGroup(String),
    /// A file operation failed.
    #[error("{}: {source}", path.display())]
    Io {
        /// File the operation acted on.
        path: PathBuf,
        /// Underlying error.
        source: io::Error,
    },
    /// An external command failed.
    #[error("`{command}` failed: {message}")]
    Command {
        /// The command line.
        command: String,
        /// Exit status and stderr.
        message: String,
    },
    /// Writing the report failed.
    #[error("cannot write the setup report: {0}")]
    Report(#[from] io::Error),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> SetupError + '_ {
    move |source| SetupError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// What [`install`] sets up.
#[derive(Debug, Clone, Copy)]
pub struct InstallPlan<'a> {
    /// User the service runs as.
    pub user: &'a str,
    /// The running binary; the launcher symlink `foci-tune` in its directory must resolve to it.
    pub exe: &'a Path,
}

enum Step {
    Write {
        dest: PathBuf,
        contents: String,
        mode: u32,
    },
    Skip {
        what: String,
        reason: String,
    },
    Run {
        argv: Vec<&'static str>,
        tolerate_failure: bool,
    },
    RunAs {
        argv: Vec<String>,
    },
    Remove {
        path: PathBuf,
    },
}

impl Step {
    fn run(argv: &[&'static str]) -> Self {
        Step::Run {
            argv: argv.to_vec(),
            tolerate_failure: false,
        }
    }
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::Write { dest, mode, .. } => {
                write!(f, "write {} (mode {mode:04o})", dest.display())
            }
            Step::Skip { what, reason } => write!(f, "{what} (skip: {reason})"),
            Step::Run {
                argv,
                tolerate_failure,
            } => {
                f.write_str(&argv.join(" "))?;
                if *tolerate_failure {
                    f.write_str(" (failure tolerated)")?;
                }
                Ok(())
            }
            Step::RunAs { argv } => f.write_str(&argv.join(" ")),
            Step::Remove { path } => write!(f, "remove {} (if present)", path.display()),
        }
    }
}

enum Outcome {
    Done,
    Unchanged,
    Skipped(String),
}

/// Installs the udev rule and a unit running the launcher symlink for `plan.user`, then reloads
/// udev and enables the service. The binary itself is not copied.
///
/// # Errors
///
/// See [`SetupError`]. Host, user, group and install-directory problems are reported before
/// anything changes.
pub fn install(
    setup: &Setup,
    plan: &InstallPlan<'_>,
    runner: &mut dyn Runner,
    out: &mut dyn Write,
) -> Result<(), SetupError> {
    let user = plan.user;
    validate_user(user)?;
    setup.check_host("sudo ~/foci-tune/foci-tune setup")?;
    let launcher = launcher_path(plan.exe)?;
    let member = group_membership(setup, user)?;

    let mut steps = Vec::new();
    steps.push(Step::Write {
        dest: setup.path(RULE_PATH),
        contents: files::udev_rule(),
        mode: 0o644,
    });
    steps.push(Step::Write {
        dest: setup.path(UNIT_PATH),
        contents: files::unit(user, &launcher),
        mode: 0o644,
    });
    let usermod = vec![
        "usermod".to_string(),
        "-aG".into(),
        files::GROUP.into(),
        "--".into(),
        user.into(),
    ];
    if member {
        steps.push(Step::Skip {
            what: usermod.join(" "),
            reason: format!("{user} is already in {}", files::GROUP),
        });
    } else {
        steps.push(Step::RunAs { argv: usermod });
    }
    steps.push(Step::run(&["udevadm", "control", "--reload"]));
    steps.push(Step::run(&["udevadm", "trigger"]));
    steps.push(Step::run(&["systemctl", "daemon-reload"]));
    steps.push(Step::run(&["systemctl", "enable", "--now", files::SERVICE]));

    let was_enabled = fs::symlink_metadata(setup.path(ENABLED_LINK_PATH)).is_ok();
    writeln!(out, "foci-tune setup for user {user}:")?;
    let changed = perform(setup, &steps, runner, out)?;
    if was_enabled && changed.get(1) == Some(&true) {
        writeln!(
            out,
            "foci-tune was already enabled and keeps running the old unit; run \
             `sudo systemctl restart foci-tune` when no tuning run is active."
        )?;
    }
    if !setup.dry_run && !member {
        writeln!(
            out,
            "{user} was added to {}; it takes effect for new logins and for the service.",
            files::GROUP
        )?;
    }
    Ok(())
}

/// Stops and disables the service and removes the files [`install`] writes.
///
/// Group membership and the `foci-results` data directory are left in place.
///
/// # Errors
///
/// See [`SetupError`]. A failing `systemctl disable` (such as a unit that is not loaded) is
/// reported and tolerated.
pub fn remove(
    setup: &Setup,
    runner: &mut dyn Runner,
    out: &mut dyn Write,
) -> Result<(), SetupError> {
    setup.check_host("sudo ~/foci-tune/foci-tune setup --remove")?;
    let steps = [
        Step::Run {
            argv: vec!["systemctl", "disable", "--now", files::SERVICE],
            tolerate_failure: true,
        },
        Step::Remove {
            path: setup.path(UNIT_PATH),
        },
        Step::Remove {
            path: setup.path(RULE_PATH),
        },
        Step::run(&["systemctl", "daemon-reload"]),
        Step::run(&["udevadm", "control", "--reload"]),
    ];

    writeln!(out, "foci-tune setup --remove:")?;
    perform(setup, &steps, runner, out)?;
    writeln!(
        out,
        "Left in place: {} group memberships and the foci-results data directory under \
         Moonraker's config root.",
        files::GROUP
    )?;
    let drop_in = setup.path(DROP_IN_PATH);
    if drop_in.exists() {
        writeln!(
            out,
            "Also left in place: the drop-in directory {}.",
            drop_in.display()
        )?;
    }
    Ok(())
}

fn perform(
    setup: &Setup,
    steps: &[Step],
    runner: &mut dyn Runner,
    out: &mut dyn Write,
) -> Result<Vec<bool>, SetupError> {
    writeln!(out, "plan:")?;
    for (i, step) in steps.iter().enumerate() {
        writeln!(out, "  {}. {step}", i + 1)?;
        if let Step::Write { contents, .. } = step {
            for line in contents.lines() {
                writeln!(out, "       | {line}")?;
            }
        }
    }
    if setup.dry_run {
        writeln!(out, "dry run: nothing was changed.")?;
        return Ok(Vec::new());
    }
    let mut changed = Vec::with_capacity(steps.len());
    for step in steps {
        let outcome = execute(step, runner)?;
        changed.push(matches!(outcome, Outcome::Done));
        let label = match outcome {
            Outcome::Done => "done".to_string(),
            Outcome::Unchanged => "unchanged".to_string(),
            Outcome::Skipped(_) if matches!(step, Step::Skip { .. }) => "skipped".to_string(),
            Outcome::Skipped(reason) => format!("skipped ({reason})"),
        };
        writeln!(out, "  {step}: {label}")?;
    }
    Ok(changed)
}

fn execute(step: &Step, runner: &mut dyn Runner) -> Result<Outcome, SetupError> {
    match step {
        Step::Write {
            dest,
            contents,
            mode,
        } => write_atomic(dest, contents.as_bytes(), *mode),
        Step::Skip { reason, .. } => Ok(Outcome::Skipped(reason.clone())),
        Step::Run {
            argv,
            tolerate_failure,
        } => run_command(runner, argv, *tolerate_failure),
        Step::RunAs { argv } => {
            let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
            run_command(runner, &argv, false)
        }
        Step::Remove { path } => match fs::remove_file(path) {
            Ok(()) => Ok(Outcome::Done),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Ok(Outcome::Skipped("not present".into()))
            }
            Err(e) => Err(io_err(path)(e)),
        },
    }
}

fn run_command(
    runner: &mut dyn Runner,
    argv: &[&str],
    tolerate_failure: bool,
) -> Result<Outcome, SetupError> {
    match runner.run(argv) {
        Ok(CommandOutcome::Ran) => Ok(Outcome::Done),
        Ok(CommandOutcome::NotExecuted) => Ok(Outcome::Skipped("not executed under --root".into())),
        Err(message) if tolerate_failure => Ok(Outcome::Skipped(message)),
        Err(message) => Err(SetupError::Command {
            command: argv.join(" "),
            message,
        }),
    }
}

/// Writes `bytes` to `dest` with `mode` through a temporary file renamed over it, so a reader
/// never sees a partial file; leaves an identical file untouched.
fn write_atomic(dest: &Path, bytes: &[u8], mode: u32) -> Result<Outcome, SetupError> {
    if let (Ok(existing), Ok(meta)) = (fs::read(dest), fs::metadata(dest))
        && existing == bytes
        && meta.permissions().mode() & 0o7777 == mode
    {
        return Ok(Outcome::Unchanged);
    }
    let name = dest
        .file_name()
        .map_or_else(|| "foci-tune".into(), |n| n.to_string_lossy().into_owned());
    let tmp = dest.with_file_name(format!(".{name}.foci-tune-setup"));
    let written = (|| {
        let mut file = open_temp(&tmp, mode)?;
        file.write_all(bytes)?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
        fs::rename(&tmp, dest)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(io_err(dest)(e));
    }
    Ok(Outcome::Done)
}

/// Creates the temp file with `mode` from the start, so it is never more open than the final
/// file, and replaces (never writes through) a stale file or symlink left at that path.
fn open_temp(tmp: &Path, mode: u32) -> io::Result<fs::File> {
    match fs::remove_file(tmp) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(libc::O_NOFOLLOW)
        .open(tmp)
}

/// Picks the service user: `explicit` (`--user`) if given, else `sudo_user` (`SUDO_USER`).
///
/// # Errors
/// [`SetupError::NoUser`] if neither is set, [`SetupError::RootUser`] if the chosen name is
/// `root`, [`SetupError::InvalidUser`] if it is not a plain account name.
pub fn resolve_user(explicit: Option<&str>, sudo_user: Option<&str>) -> Result<String, SetupError> {
    let user = explicit.or(sudo_user).ok_or(SetupError::NoUser)?;
    if user == "root" {
        return Err(SetupError::RootUser);
    }
    validate_user(user)?;
    Ok(user.to_owned())
}

/// Accepts only a plain account name (`[a-z_][a-z0-9_-]*$?`, at most 32 bytes), so the name
/// cannot add lines or specifiers to the unit or be taken as an option by `usermod`.
fn validate_user(user: &str) -> Result<(), SetupError> {
    let body = user.strip_suffix('$').unwrap_or(user);
    let mut chars = body.chars();
    let valid = user.len() <= 32
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if valid {
        Ok(())
    } else {
        Err(SetupError::InvalidUser(user.into()))
    }
}

/// The `foci-tune` symlink beside `exe`, after checking it resolves to `exe` and that its
/// directory is safe to write into a unit's `ExecStart=` line.
fn launcher_path(exe: &Path) -> Result<String, SetupError> {
    let dir = exe.parent().unwrap_or(Path::new("/"));
    let bad_dir = |reason| SetupError::BadInstallDir {
        path: dir.to_path_buf(),
        reason,
    };
    let dir_text = dir
        .to_str()
        .ok_or_else(|| bad_dir("it is not valid UTF-8"))?;
    if dir_text
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '%' | '$' | '"' | '\'' | '\\'))
    {
        return Err(bad_dir(
            "it contains whitespace or a character systemd treats specially",
        ));
    }
    let launcher = dir.join(LAUNCHER_NAME);
    let is_symlink = fs::symlink_metadata(&launcher).is_ok_and(|m| m.file_type().is_symlink());
    let resolves_to_exe = matches!(
        (fs::canonicalize(&launcher), fs::canonicalize(exe)),
        (Ok(l), Ok(e)) if l == e
    );
    if is_symlink && resolves_to_exe {
        Ok(format!("{dir_text}/{LAUNCHER_NAME}"))
    } else {
        Err(SetupError::MissingLauncher(launcher))
    }
}

/// Whether `user` is in the group, by `/etc/group` member list or primary group.
fn group_membership(setup: &Setup, user: &str) -> Result<bool, SetupError> {
    let passwd_path = setup.path("etc/passwd");
    let passwd = fs::read_to_string(&passwd_path).map_err(io_err(&passwd_path))?;
    let primary_gid = passwd
        .lines()
        .map(|l| l.split(':').collect::<Vec<_>>())
        .find(|f| f.first() == Some(&user))
        .and_then(|f| f.get(3).map(|g| g.to_string()))
        .ok_or_else(|| SetupError::UnknownUser(user.into()))?;

    let group_path = setup.path("etc/group");
    let groups = fs::read_to_string(&group_path).map_err(io_err(&group_path))?;
    let fields = groups
        .lines()
        .map(|l| l.split(':').collect::<Vec<_>>())
        .find(|f| f.first() == Some(&files::GROUP))
        .ok_or_else(|| SetupError::MissingGroup(files::GROUP.into()))?;
    let listed = fields
        .get(3)
        .is_some_and(|m| m.split(',').any(|m| m == user));
    Ok(listed || fields.get(2) == Some(&primary_gid.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_temp_file_never_has_more_permission_bits_than_its_final_mode() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join(".rule.foci-tune-setup");

        let file = open_temp(&tmp, 0o600).unwrap();

        let mode = file.metadata().unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode & !0o600, 0, "temp created with {mode:04o}");
    }
}
