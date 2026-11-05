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

#[test]
fn setup_under_a_root_prefix_dry_runs_then_installs_without_running_commands() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for d in [
        "etc/udev/rules.d",
        "etc/systemd/system",
        "home/pi/foci-tune",
    ] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::write(
        root.join("etc/passwd"),
        "pi:x:1000:1000::/home/pi:/bin/bash\n",
    )
    .unwrap();
    std::fs::write(root.join("etc/group"), "pi:x:1000:\nplugdev:x:46:\n").unwrap();
    let unit = root.join("etc/systemd/system/foci-tune.service");
    let exe = root.join("home/pi/foci-tune/foci-tune-aarch64");
    std::fs::copy(assert_cmd::cargo::cargo_bin("foci-tune"), &exe).unwrap();
    std::os::unix::fs::symlink(
        "foci-tune-aarch64",
        root.join("home/pi/foci-tune/foci-tune"),
    )
    .unwrap();
    let setup = |extra: &[&str]| {
        let mut cmd = Command::new(&exe);
        cmd.args(["setup", "--root"]).arg(root).args(extra);
        cmd
    };

    setup(&["--dry-run", "--user", "pi"])
        .assert()
        .success()
        .stdout(predicates::str::contains("User=pi"))
        .stdout(predicates::str::contains("dry run"));
    assert!(!unit.exists());

    setup(&[])
        .assert()
        .success()
        .stdout(predicates::str::contains("not executed under --root"));
    assert!(
        std::fs::read_to_string(&unit)
            .unwrap()
            .contains("User=pi\n")
    );
    assert!(root.join("etc/udev/rules.d/99-foci-trace.rules").is_file());
}
