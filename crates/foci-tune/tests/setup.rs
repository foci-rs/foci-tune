use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use foci_trace::usb::{TRACE_PRODUCT_ID, TRACE_VENDOR_ID};
use foci_tune::setup::{
    CommandOutcome, Host, InstallPlan, Runner, Setup, SetupError, install, remove, resolve_user,
};

const DIR: &str = "home/pi/foci-tune";
const ARCH_BIN: &str = "home/pi/foci-tune/foci-tune-aarch64";
const LAUNCHER: &str = "home/pi/foci-tune/foci-tune";
const RULE: &str = "etc/udev/rules.d/99-foci-trace.rules";
const UNIT: &str = "etc/systemd/system/foci-tune.service";
const LINUX_ROOT: Host = Host {
    euid: 0,
    linux: true,
};

#[derive(Default)]
struct Recorder {
    commands: Vec<String>,
    fail: Option<&'static str>,
}

impl Runner for Recorder {
    fn run(&mut self, argv: &[&str]) -> Result<CommandOutcome, String> {
        let line = argv.join(" ");
        self.commands.push(line.clone());
        if self.fail == Some(line.as_str()) {
            return Err("Failed to disable unit: Unit foci-tune.service not loaded.".into());
        }
        Ok(CommandOutcome::Ran)
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    exe: PathBuf,
}

impl Fixture {
    fn new(plugdev: Option<&str>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        for d in ["etc/udev/rules.d", "etc/systemd/system", DIR, "download"] {
            fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        fs::write(
            dir.path().join("etc/passwd"),
            "root:x:0:0::/root:/bin/sh\npi:x:1000:1000::/home/pi:/bin/bash\n\
             klipper:x:1001:1001::/home/klipper:/bin/bash\n",
        )
        .unwrap();
        let mut group = String::from("root:x:0:\npi:x:1000:\nklipper:x:1001:\n");
        if let Some(line) = plugdev {
            group.push_str(line);
            group.push('\n');
        }
        fs::write(dir.path().join("etc/group"), group).unwrap();
        let exe = dir.path().join(ARCH_BIN);
        fs::write(&exe, b"new binary").unwrap();
        std::os::unix::fs::symlink("foci-tune-aarch64", dir.path().join(LAUNCHER)).unwrap();
        Self { dir, exe }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn setup(&self, dry_run: bool) -> Setup {
        Setup {
            prefix: Some(self.root().to_path_buf()),
            dry_run,
            host: LINUX_ROOT,
        }
    }

    fn install(&self, user: &str, runner: &mut Recorder) -> Result<String, SetupError> {
        let mut out = Vec::new();
        let plan = InstallPlan {
            user,
            exe: &self.exe,
        };
        install(&self.setup(false), &plan, runner, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    fn remove(&self, runner: &mut Recorder) -> Result<String, SetupError> {
        let mut out = Vec::new();
        remove(&self.setup(false), runner, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found.sort();
    found
}

#[test]
fn install_writes_the_rule_and_unit_and_runs_the_commands_in_order() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    let mut runner = Recorder::default();

    fx.install("klipper", &mut runner).unwrap();

    let rule = fs::read_to_string(fx.path(RULE)).unwrap();
    assert!(rule.contains(&format!("ATTR{{idVendor}}==\"{TRACE_VENDOR_ID:04x}\"")));
    assert!(rule.contains(&format!("ATTR{{idProduct}}==\"{TRACE_PRODUCT_ID:04x}\"")));
    assert!(rule.contains("GROUP=\"plugdev\""));
    assert_eq!(mode(&fx.path(RULE)), 0o644);
    let unit = fs::read_to_string(fx.path(UNIT)).unwrap();
    let launcher = fx.path(LAUNCHER).display().to_string();
    let service: Vec<&str> = unit
        .lines()
        .filter(|l| l.starts_with("User=") || l.starts_with("ExecStart"))
        .collect();
    assert_eq!(
        service,
        [
            "User=klipper".to_string(),
            format!("ExecStartPre={launcher} --check"),
            format!(
                "ExecStart={launcher} --moonraker-url http://127.0.0.1:7125 --listen 0.0.0.0:7130"
            ),
        ]
    );
    assert_eq!(mode(&fx.path(UNIT)), 0o644);
    assert_eq!(
        runner.commands,
        [
            "usermod -aG plugdev -- klipper",
            "udevadm control --reload",
            "udevadm trigger",
            "systemctl daemon-reload",
            "systemctl enable --now foci-tune",
        ]
    );
}

#[test]
fn a_second_install_reports_unchanged_rewrites_nothing_and_still_reloads() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    fx.install("pi", &mut Recorder::default()).unwrap();
    let inodes: Vec<u64> = [RULE, UNIT]
        .iter()
        .map(|p| fs::metadata(fx.path(p)).unwrap().ino())
        .collect();
    let mut runner = Recorder::default();

    let out = fx.install("pi", &mut runner).unwrap();

    let after: Vec<u64> = [RULE, UNIT]
        .iter()
        .map(|p| fs::metadata(fx.path(p)).unwrap().ino())
        .collect();
    assert_eq!(inodes, after);
    assert_eq!(out.matches("unchanged").count(), 2, "{out}");
    assert!(out.contains("pi is already in plugdev"), "{out}");
    assert_eq!(
        runner.commands,
        [
            "udevadm control --reload",
            "udevadm trigger",
            "systemctl daemon-reload",
            "systemctl enable --now foci-tune",
        ]
    );
}

#[test]
fn a_changed_file_is_replaced_with_the_right_mode_and_no_temp_file_left() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    fs::write(fx.path(RULE), "old rule\n").unwrap();
    fs::set_permissions(fx.path(RULE), fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(fx.path("etc/udev/rules.d/50-other.rules"), "other\n").unwrap();

    fx.install("pi", &mut Recorder::default()).unwrap();

    assert!(
        fs::read_to_string(fx.path(RULE))
            .unwrap()
            .contains("GROUP=\"plugdev\"")
    );
    assert_eq!(mode(&fx.path(RULE)), 0o644);
    assert_eq!(
        files_under(&fx.path("etc/udev/rules.d")),
        [fx.path("etc/udev/rules.d/50-other.rules"), fx.path(RULE)]
    );
}

#[test]
fn install_writes_the_unit_pointing_at_the_launcher_symlink_not_the_arch_binary() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));

    fx.install("pi", &mut Recorder::default()).unwrap();

    let unit = fs::read_to_string(fx.path(UNIT)).unwrap();
    let launcher = fx.path(LAUNCHER).display().to_string();
    assert!(unit.contains(&format!("ExecStart={launcher} ")), "{unit}");
    assert!(!unit.contains("foci-tune-aarch64"), "{unit}");
    assert!(!unit.contains("usr/local/bin"), "{unit}");
}

#[test]
fn install_does_not_copy_the_binary_anywhere() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    let before = files_under(&fx.path("home"));

    fx.install("pi", &mut Recorder::default()).unwrap();

    assert_eq!(files_under(&fx.path("home")), before);
    assert_eq!(fs::read(fx.path(ARCH_BIN)).unwrap(), b"new binary");
    assert!(!fx.path("usr").exists());
    assert_eq!(
        files_under(&fx.path("etc"))
            .into_iter()
            .filter(|p| p.starts_with(fx.path("etc/udev")) || p.starts_with(fx.path("etc/systemd")))
            .collect::<Vec<_>>(),
        [fx.path(UNIT), fx.path(RULE)]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_missing_launcher_symlink_fails_before_anything_is_written_or_run() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    fs::remove_file(fx.path(LAUNCHER)).unwrap();
    let mut runner = Recorder::default();

    let err = fx.install("pi", &mut runner).unwrap_err();

    assert!(matches!(err, SetupError::MissingLauncher(_)), "{err:?}");
    assert!(!fx.path(RULE).exists());
    assert!(!fx.path(UNIT).exists());
    assert!(runner.commands.is_empty());
}

#[test]
fn a_launcher_pointing_at_a_different_file_fails_before_anything_is_written_or_run() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    fs::write(fx.path("home/pi/foci-tune/foci-tune-armv7"), b"other").unwrap();
    fs::remove_file(fx.path(LAUNCHER)).unwrap();
    std::os::unix::fs::symlink("foci-tune-armv7", fx.path(LAUNCHER)).unwrap();
    let mut runner = Recorder::default();

    let err = fx.install("pi", &mut runner).unwrap_err();

    assert!(matches!(err, SetupError::MissingLauncher(_)), "{err:?}");
    assert!(!fx.path(RULE).exists());
    assert!(!fx.path(UNIT).exists());
    assert!(runner.commands.is_empty());
}

#[test]
fn an_install_dir_with_whitespace_or_systemd_specials_is_rejected() {
    for bad in ["a b", "a%h", "a$b", "a'b", "a\"b", "a\\b"] {
        let fx = Fixture::new(Some("plugdev:x:46:pi"));
        let dir = fx.path("home/pi").join(bad);
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("foci-tune-aarch64");
        fs::write(&exe, b"bin").unwrap();
        std::os::unix::fs::symlink("foci-tune-aarch64", dir.join("foci-tune")).unwrap();
        let mut runner = Recorder::default();
        let plan = InstallPlan {
            user: "pi",
            exe: &exe,
        };

        let err = install(&fx.setup(false), &plan, &mut runner, &mut Vec::new()).unwrap_err();

        assert!(
            matches!(err, SetupError::BadInstallDir { .. }),
            "{bad:?}: {err:?}"
        );
        assert!(!fx.path(RULE).exists());
        assert!(!fx.path(UNIT).exists());
        assert!(runner.commands.is_empty());
    }
}

#[test]
fn a_missing_plugdev_group_fails_before_anything_is_written_or_run() {
    let fx = Fixture::new(None);
    let mut runner = Recorder::default();

    let err = fx.install("pi", &mut runner).unwrap_err();

    assert!(matches!(err, SetupError::MissingGroup(_)), "{err:?}");
    assert!(err.to_string().contains("plugdev"), "{err}");
    assert!(!fx.path(RULE).exists());
    assert!(!fx.path(UNIT).exists());
    assert!(runner.commands.is_empty());
}

#[test]
fn an_unknown_user_fails_before_anything_is_written_or_run() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    let mut runner = Recorder::default();

    let err = fx.install("mainsail", &mut runner).unwrap_err();

    assert!(matches!(err, SetupError::UnknownUser(_)), "{err:?}");
    assert!(!fx.path(UNIT).exists());
    assert!(runner.commands.is_empty());
}

#[test]
fn a_dry_run_prints_paths_modes_contents_and_commands_and_changes_nothing() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    let mut runner = Recorder::default();
    let mut out = Vec::new();

    let plan = InstallPlan {
        user: "klipper",
        exe: &fx.exe,
    };
    install(&fx.setup(true), &plan, &mut runner, &mut out).unwrap();

    let out = String::from_utf8(out).unwrap();
    for expected in [
        fx.path(RULE).display().to_string(),
        fx.path(UNIT).display().to_string(),
        "0644".into(),
        "User=klipper".into(),
        "WantedBy=multi-user.target".into(),
        format!("ATTR{{idVendor}}==\"{TRACE_VENDOR_ID:04x}\""),
        "usermod -aG plugdev -- klipper".into(),
        "systemctl enable --now foci-tune".into(),
    ] {
        assert!(out.contains(&expected), "missing {expected:?} in:\n{out}");
    }
    assert!(files_under(&fx.path("etc/udev")).is_empty());
    assert!(files_under(&fx.path("etc/systemd")).is_empty());
    assert!(runner.commands.is_empty());
}

#[test]
fn a_real_run_without_root_is_refused_with_a_sudo_hint_before_anything_happens() {
    let setup = Setup {
        prefix: None,
        dry_run: false,
        host: Host {
            euid: 1000,
            linux: true,
        },
    };
    let mut runner = Recorder::default();

    let plan = InstallPlan {
        user: "pi",
        exe: Path::new("/nonexistent"),
    };
    let install_err = install(&setup, &plan, &mut runner, &mut Vec::new()).unwrap_err();
    let remove_err = remove(&setup, &mut runner, &mut Vec::new()).unwrap_err();

    assert!(
        matches!(install_err, SetupError::NotRoot(_)),
        "{install_err:?}"
    );
    assert!(
        matches!(remove_err, SetupError::NotRoot(_)),
        "{remove_err:?}"
    );
    assert!(runner.commands.is_empty());
}

#[test]
fn remove_deletes_exactly_the_installed_files_and_leaves_the_rest() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    fx.install("pi", &mut Recorder::default()).unwrap();
    let keep = [
        "etc/udev/rules.d/50-other.rules",
        "etc/systemd/system/moonraker.service",
        "home/pi/foci-tune/foci-tune-aarch64.keep",
        "home/pi/printer_data/config/foci-results/sessions/s1.json",
        "etc/systemd/system/foci-tune.service.d/override.conf",
    ];
    for k in keep {
        fs::create_dir_all(fx.path(k).parent().unwrap()).unwrap();
        fs::write(fx.path(k), "keep").unwrap();
    }
    let group_before = fs::read(fx.path("etc/group")).unwrap();
    let mut runner = Recorder::default();

    let out = fx.remove(&mut runner).unwrap();

    for p in [RULE, UNIT] {
        assert!(!fx.path(p).exists(), "{p} still present");
    }
    for k in keep {
        assert_eq!(fs::read_to_string(fx.path(k)).unwrap(), "keep");
    }
    assert_eq!(fs::read(fx.path("etc/group")).unwrap(), group_before);
    assert_eq!(
        runner.commands,
        [
            "systemctl disable --now foci-tune",
            "systemctl daemon-reload",
            "udevadm control --reload",
        ]
    );
    assert!(out.contains("foci-results"), "{out}");
    assert!(out.contains("plugdev"), "{out}");
    assert!(
        out.contains(
            &fx.path("etc/systemd/system/foci-tune.service.d")
                .display()
                .to_string()
        ),
        "{out}"
    );
}

#[test]
fn remove_with_nothing_installed_tolerates_an_unloaded_unit() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    let mut runner = Recorder {
        fail: Some("systemctl disable --now foci-tune"),
        ..Recorder::default()
    };

    let out = fx.remove(&mut runner).unwrap();

    assert!(out.contains("not loaded"), "{out}");
    assert_eq!(out.matches("not present").count(), 2, "{out}");
    assert_eq!(
        runner.commands,
        [
            "systemctl disable --now foci-tune",
            "systemctl daemon-reload",
            "udevadm control --reload",
        ]
    );
}

#[test]
fn a_remove_dry_run_changes_nothing() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    fx.install("pi", &mut Recorder::default()).unwrap();
    let mut runner = Recorder::default();
    let mut out = Vec::new();

    remove(&fx.setup(true), &mut runner, &mut out).unwrap();

    for p in [RULE, UNIT] {
        assert!(fx.path(p).exists(), "{p} removed by a dry run");
    }
    assert!(runner.commands.is_empty());
    let out = String::from_utf8(out).unwrap();
    assert!(out.contains("systemctl disable --now foci-tune"), "{out}");
}

#[test]
fn a_stale_temp_file_or_symlink_is_replaced_without_writing_through_it() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    let victim = fx.path("download/victim");
    fs::write(&victim, "victim").unwrap();
    std::os::unix::fs::symlink(
        &victim,
        fx.path("etc/udev/rules.d/.99-foci-trace.rules.foci-tune-setup"),
    )
    .unwrap();
    let stale = fx.path("etc/systemd/system/.foci-tune.service.foci-tune-setup");
    fs::write(&stale, "stale").unwrap();
    fs::set_permissions(&stale, fs::Permissions::from_mode(0o666)).unwrap();

    fx.install("pi", &mut Recorder::default()).unwrap();

    assert_eq!(fs::read_to_string(&victim).unwrap(), "victim");
    let rule_meta = fs::symlink_metadata(fx.path(RULE)).unwrap();
    assert!(rule_meta.file_type().is_file());
    assert!(
        fs::read_to_string(fx.path(RULE))
            .unwrap()
            .contains("GROUP=\"plugdev\"")
    );
    assert_eq!(mode(&fx.path(UNIT)), 0o644);
    assert!(
        fs::read_to_string(fx.path(UNIT))
            .unwrap()
            .contains("User=pi")
    );
    assert_eq!(
        files_under(&fx.path("etc")),
        [
            fx.path("etc/group"),
            fx.path("etc/passwd"),
            fx.path(UNIT),
            fx.path(RULE)
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );
}

#[test]
fn a_user_name_that_is_not_a_plain_account_name_is_rejected_before_anything_happens() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    let long = "a".repeat(33);
    for bad in [
        "pi\nExecStart=/bin/sh",
        "-o",
        "%i",
        "",
        long.as_str(),
        "Pi",
        "pi user",
    ] {
        let mut runner = Recorder::default();

        let err = fx.install(bad, &mut runner).unwrap_err();

        assert!(
            matches!(err, SetupError::InvalidUser(_)),
            "{bad:?}: {err:?}"
        );
        assert!(runner.commands.is_empty());
        assert!(!fx.path(UNIT).exists());
    }
}

#[test]
fn plain_account_names_are_accepted() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    let mut passwd = fs::read_to_string(fx.path("etc/passwd")).unwrap();
    passwd.push_str("foci-svc:x:1002:1002::/:/bin/false\n_x1:x:1003:1003::/:/bin/false\n");
    fs::write(fx.path("etc/passwd"), passwd).unwrap();
    let mut runner = Recorder::default();

    for good in ["pi", "foci-svc", "_x1"] {
        fx.install(good, &mut runner).unwrap();
    }

    assert!(
        runner
            .commands
            .contains(&"usermod -aG plugdev -- foci-svc".to_string())
    );
}

#[test]
fn a_restart_hint_is_printed_only_when_an_enabled_service_got_a_new_unit() {
    let fx = Fixture::new(Some("plugdev:x:46:pi"));
    let hint = "sudo systemctl restart foci-tune";

    let first = fx.install("pi", &mut Recorder::default()).unwrap();
    let wants = fx.path("etc/systemd/system/multi-user.target.wants");
    fs::create_dir_all(&wants).unwrap();
    std::os::unix::fs::symlink(fx.path(UNIT), wants.join("foci-tune.service")).unwrap();
    let unchanged = fx.install("pi", &mut Recorder::default()).unwrap();
    let upgraded = fx.install("klipper", &mut Recorder::default()).unwrap();

    assert!(!first.contains(hint), "{first}");
    assert!(!unchanged.contains(hint), "{unchanged}");
    assert!(upgraded.contains(hint), "{upgraded}");
}

#[test]
fn resolve_user_prefers_the_explicit_flag() {
    assert_eq!(
        resolve_user(Some("klipper"), Some("pi")).unwrap(),
        "klipper"
    );
}

#[test]
fn resolve_user_falls_back_to_sudo_user() {
    assert_eq!(resolve_user(None, Some("pi")).unwrap(), "pi");
}

#[test]
fn no_flag_and_no_sudo_user_is_refused() {
    assert!(matches!(resolve_user(None, None), Err(SetupError::NoUser)));
}

#[test]
fn root_is_refused_from_either_source() {
    assert!(matches!(
        resolve_user(Some("root"), Some("pi")),
        Err(SetupError::RootUser)
    ));
    assert!(matches!(
        resolve_user(None, Some("root")),
        Err(SetupError::RootUser)
    ));
}

#[test]
fn a_malformed_sudo_user_is_refused() {
    assert!(matches!(
        resolve_user(None, Some("pi\nExecStart=/bin/sh")),
        Err(SetupError::InvalidUser(_))
    ));
}
