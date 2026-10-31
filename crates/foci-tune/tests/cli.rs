use assert_cmd::Command;

#[test]
fn check_flag_exits_zero_without_network_or_usb() {
    Command::cargo_bin("foci-tune")
        .unwrap()
        .arg("--check")
        .assert()
        .success();
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
