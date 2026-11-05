use assert_cmd::Command;

#[test]
fn check_flag_exits_zero_without_network_or_usb() {
    Command::cargo_bin("foci-tune")
        .unwrap()
        .arg("--check")
        .assert()
        .success()
        .stdout(predicates::str::contains("index.html"));
}

#[test]
fn version_flag_prints_the_crate_version() {
    Command::cargo_bin("foci-tune")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::contains(env!("CARGO_PKG_VERSION")));
}

mod fake_moonraker;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_service_refuses_to_start_without_a_writable_data_directory() {
    let dir = tempfile::tempdir().unwrap();
    let not_a_dir = dir.path().join("file");
    std::fs::write(&not_a_dir, b"").unwrap();
    let fake = fake_moonraker::FakeMoonraker::start().await;
    fake.set_config_root(not_a_dir.join("config").to_str().unwrap());
    let listen = format!("127.0.0.1:{}", free_port());
    let url = fake.url();

    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(assert_cmd::cargo::cargo_bin("foci-tune"))
            .args(["--moonraker-url", &url, "--listen", &listen])
            .output()
            .unwrap()
    })
    .await
    .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("is not writable"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_service_serves_the_api_and_stops_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_moonraker::FakeMoonraker::start().await;
    fake.set_config_root(dir.path().to_str().unwrap());
    let port = free_port();
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("foci-tune"))
        .args(["--moonraker-url", &fake.url(), "--listen"])
        .arg(format!("127.0.0.1:{port}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let http = reqwest::Client::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let printer = loop {
        if let Ok(r) = http
            .get(format!("http://127.0.0.1:{port}/api/printer"))
            .send()
            .await
        {
            break r;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "service never listened"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    let index = http
        .get(format!("http://127.0.0.1:{port}/"))
        .send()
        .await
        .unwrap();
    // SAFETY: `child.id()` is the pid of a child this test spawned and has not reaped.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    let status = tokio::task::spawn_blocking(move || child.wait().unwrap())
        .await
        .unwrap();

    assert_eq!(printer.status(), 200);
    let body: serde_json::Value = printer.json().await.unwrap();
    assert_eq!(body["moonraker"]["klippy"], "ready");
    assert_eq!(index.status(), 200);
    assert!(dir.path().join("foci-results/sessions").is_dir());
    assert!(status.success(), "{status:?}");
}

struct PrefixedRoot {
    dir: tempfile::TempDir,
    exe: std::path::PathBuf,
}

impl PrefixedRoot {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for d in [
            "etc/udev/rules.d",
            "etc/systemd/system",
            "home/pi/foci-tune",
            "home/pi/printer_data",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(
            root.join("etc/passwd"),
            "pi:x:1000:1000::/home/pi:/bin/bash\n",
        )
        .unwrap();
        std::fs::write(root.join("etc/group"), "pi:x:1000:\nplugdev:x:46:\n").unwrap();
        std::fs::write(
            root.join("home/pi/printer_data/moonraker.asvc"),
            "klipper\n",
        )
        .unwrap();
        let exe = root.join("home/pi/foci-tune/foci-tune-aarch64");
        std::fs::copy(assert_cmd::cargo::cargo_bin("foci-tune"), &exe).unwrap();
        std::os::unix::fs::symlink(
            "foci-tune-aarch64",
            root.join("home/pi/foci-tune/foci-tune"),
        )
        .unwrap();
        Self { dir, exe }
    }

    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.dir.path().join(rel)
    }

    fn asvc(&self) -> std::path::PathBuf {
        self.path("home/pi/printer_data/moonraker.asvc")
    }

    fn unit(&self) -> std::path::PathBuf {
        self.path("etc/systemd/system/foci-tune.service")
    }

    fn setup(&self, extra: &[&str]) -> std::process::Command {
        let mut cmd = std::process::Command::new(&self.exe);
        cmd.args(["setup", "--root"])
            .arg(self.dir.path())
            .args(extra);
        cmd
    }
}

#[test]
fn setup_under_a_root_prefix_dry_runs_then_installs_without_running_commands() {
    let fx = PrefixedRoot::new();
    let asvc = fx.asvc();
    let asvc = asvc.to_str().unwrap();
    let run = |extra: &[&str]| {
        let mut cmd = Command::from_std(fx.setup(&["--asvc-path", asvc]));
        cmd.args(extra);
        cmd
    };

    run(&["--dry-run", "--user", "pi"])
        .assert()
        .success()
        .stdout(predicates::str::contains("User=pi"))
        .stdout(predicates::str::contains("dry run"));
    assert!(!fx.unit().exists());

    run(&["--user", "pi"])
        .assert()
        .success()
        .stdout(predicates::str::contains("not executed under --root"));
    assert!(
        std::fs::read_to_string(fx.unit())
            .unwrap()
            .contains("User=pi\n")
    );
    assert!(fx.path("etc/udev/rules.d/99-foci-trace.rules").is_file());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asvc_path_flag_overrides_the_moonraker_lookup_for_install_and_remove() {
    let fx = PrefixedRoot::new();
    let fake = fake_moonraker::FakeMoonraker::start().await;
    let decoy = fx.path("decoy/logs");
    fake.set_logs_root(Some(decoy.to_str().unwrap()));
    let url = fake.url();
    let asvc = fx.asvc();
    let run = |extra: Vec<String>| {
        let mut cmd = fx.setup(&[]);
        cmd.args(extra);
        tokio::task::spawn_blocking(move || cmd.output().unwrap())
    };
    let flags = |more: &[&str]| -> Vec<String> {
        [
            "--asvc-path",
            asvc.to_str().unwrap(),
            "--moonraker-url",
            &url,
            "--user",
            "pi",
        ]
        .iter()
        .chain(more)
        .map(|s| s.to_string())
        .collect()
    };

    let installed = run(flags(&[])).await.unwrap();
    assert!(installed.status.success(), "{installed:?}");
    let after_install = std::fs::read_to_string(&asvc).unwrap();
    let removed = run(vec![
        "--remove".into(),
        "--asvc-path".into(),
        asvc.to_str().unwrap().into(),
        "--moonraker-url".into(),
        url.clone(),
    ])
    .await
    .unwrap();

    assert_eq!(after_install, "klipper\nfoci-tune\n");
    assert!(removed.status.success(), "{removed:?}");
    assert_eq!(std::fs::read_to_string(&asvc).unwrap(), "klipper\n");
    assert!(!decoy.parent().unwrap().exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setup_locates_the_asvc_beside_the_logs_root_when_no_flag_is_given() {
    let fx = PrefixedRoot::new();
    let fake = fake_moonraker::FakeMoonraker::start().await;
    let logs = fx.path("home/pi/printer_data/logs");
    fake.set_logs_root(Some(logs.to_str().unwrap()));
    let mut cmd = fx.setup(&["--user", "pi", "--moonraker-url", &fake.url()]);

    let output = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read_to_string(fx.asvc()).unwrap(),
        "klipper\nfoci-tune\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_fails_with_no_writes_when_the_moonraker_lookup_fails_and_no_asvc_path_is_given() {
    let fx = PrefixedRoot::new();
    let fake = fake_moonraker::FakeMoonraker::start().await;
    fake.set_logs_root(None);
    let mut cmd = fx.setup(&["--user", "pi", "--moonraker-url", &fake.url()]);

    let output = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();

    assert!(!output.status.success());
    assert!(!fx.unit().exists());
    assert!(!fx.path("etc/udev/rules.d/99-foci-trace.rules").exists());
    assert_eq!(std::fs::read_to_string(fx.asvc()).unwrap(), "klipper\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_proceeds_and_reports_the_service_list_unchanged_when_the_lookup_fails() {
    let fx = PrefixedRoot::new();
    let asvc = fx.asvc();
    let installed = fx
        .setup(&["--user", "pi", "--asvc-path", asvc.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(installed.status.success(), "{installed:?}");
    let fake = fake_moonraker::FakeMoonraker::start().await;
    fake.set_logs_root(None);
    let mut cmd = fx.setup(&["--remove", "--moonraker-url", &fake.url()]);

    let output = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(!fx.unit().exists());
    assert_eq!(
        std::fs::read_to_string(&asvc).unwrap(),
        "klipper\nfoci-tune\n"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("moonraker.asvc"));
}

#[test]
fn setup_without_sudo_or_user_fails_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();

    Command::cargo_bin("foci-tune")
        .unwrap()
        .env_remove("SUDO_USER")
        .args(["setup", "--root"])
        .arg(dir.path())
        .assert()
        .failure();

    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
