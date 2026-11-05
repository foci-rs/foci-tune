setup() {
  TEST_ROOT="$(mktemp -d "${BATS_TMPDIR:-/tmp}/foci-tune-main.XXXXXX")"
  export TEST_ROOT
  export HOME="$TEST_ROOT/home"
  mkdir -p "$HOME" "$TEST_ROOT/fixture"
  : >"$TEST_ROOT/calls.log"
  export FOCI_RELEASE_API="https://api.test/releases/latest"
  export FOCI_DOWNLOAD_BASE="https://dl.test/releases/download"
  MOONRAKER_CONF="$TEST_ROOT/moonraker.conf"
  export MOONRAKER_CONF

  printf 'binary-v1' >"$TEST_ROOT/fixture/foci-tune-armv7"
  write_sums "$(digest_of "$TEST_ROOT/fixture/foci-tune-armv7")"
  printf '[server]\nhost: 0.0.0.0\n' >"$MOONRAKER_CONF"

  # shellcheck source=../kpi.sh
  source "$BATS_TEST_DIRNAME/../kpi.sh"
  source "$BATS_TEST_DIRNAME/../installer.sh"

  discover_klipper_env() { MOONRAKER_CONFIG="$MOONRAKER_CONF"; }
  check_no_active_print() {
    echo print_check >>"$TEST_ROOT/calls.log"
    local n
    n="$(grep -c '^print_check$' "$TEST_ROOT/calls.log")"
    [ "$n" != "$(cat "$TEST_ROOT/print_fail_at" 2>/dev/null)" ]
  }
  require_not_root() { :; }
  service_restart_if() { echo "restart:$1" >>"$TEST_ROOT/calls.log"; }
  uname() { echo armv7l; }
  log() { printf '%s\n' "$1" >&2; }
  curl() {
    local out="" url=""
    while [ $# -gt 0 ]; do
      case "$1" in
        -o) out="$2"; shift ;;
        -*) ;;
        *) url="$1" ;;
      esac
      shift
    done
    case "$url" in
      "$FOCI_RELEASE_API") echo release_api >>"$TEST_ROOT/calls.log"; echo '{"tag_name": "v1.2.3"}' ;;
      "$FOCI_DOWNLOAD_BASE/v1.2.3/SHA256SUMS") echo download >>"$TEST_ROOT/calls.log"; cp "$TEST_ROOT/fixture/SHA256SUMS" "$out" ;;
      "$FOCI_DOWNLOAD_BASE/v1.2.3/foci-tune-armv7") echo download >>"$TEST_ROOT/calls.log"; cp "$TEST_ROOT/fixture/foci-tune-armv7" "$out" ;;
      *) echo "unexpected:$url" >>"$TEST_ROOT/calls.log"; return 22 ;;
    esac
  }
  sudo() {
    echo "sudo:$*" >>"$TEST_ROOT/calls.log"
    [ ! -e "$TEST_ROOT/sudo_fail" ]
  }
  wrap verify foci_verify_checksum
  wrap install foci_install_files
  wrap block config_block_ensure
}
teardown() { rm -rf "$TEST_ROOT"; }

digest_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

write_sums() { printf '%s  foci-tune-armv7\n' "$1" >"$TEST_ROOT/fixture/SHA256SUMS"; }

wrap() {
  local label="$1" fn="$2"
  eval "_orig_$(declare -f "$fn")"
  eval "$fn() { echo $label >>\"\$TEST_ROOT/calls.log\"; _orig_$fn \"\$@\"; }"
}

calls() { tr '\n' ' ' <"$TEST_ROOT/calls.log" | sed 's/ $//'; }

@test "main_runs_the_steps_in_spec_order" {
  run foci_main
  [ "$status" -eq 0 ]
  [ "$(sed 's/^sudo:.*/sudo/' "$TEST_ROOT/calls.log" | tr '\n' ' ')" = \
    "print_check release_api download download verify install print_check sudo block print_check restart:moonraker " ]
  grep -qx "sudo:$HOME/foci-tune/foci-tune setup" "$TEST_ROOT/calls.log"
  [ -x "$HOME/foci-tune/foci-tune-armv7" ]
  [ "$(readlink "$HOME/foci-tune/foci-tune")" = "foci-tune-armv7" ]
  grep -q '"version": "v1.2.3"' "$HOME/foci-tune/release_info.json"
}

@test "an_active_print_aborts_before_any_download" {
  echo 1 >"$TEST_ROOT/print_fail_at"
  run foci_main
  [ "$status" -ne 0 ]
  [ "$(calls)" = "print_check" ]
  [ ! -e "$HOME/foci-tune" ]
}

@test "a_print_that_starts_before_sudo_setup_aborts_before_setup_runs" {
  echo 2 >"$TEST_ROOT/print_fail_at"
  run foci_main
  [ "$status" -ne 0 ]
  ! grep -q '^sudo:' "$TEST_ROOT/calls.log"
  ! grep -q '^block$' "$TEST_ROOT/calls.log"
  ! grep -q '^restart:' "$TEST_ROOT/calls.log"
}

@test "a_print_that_starts_before_the_moonraker_restart_aborts_before_the_restart" {
  echo 3 >"$TEST_ROOT/print_fail_at"
  run foci_main
  [ "$status" -ne 0 ]
  grep -q '^block$' "$TEST_ROOT/calls.log"
  ! grep -q '^restart:' "$TEST_ROOT/calls.log"
}

@test "an_indeterminate_print_state_aborts_like_a_print" {
  check_no_active_print() { echo print_check >>"$TEST_ROOT/calls.log"; return 1; }
  run foci_main
  [ "$status" -ne 0 ]
  [ "$(calls)" = "print_check" ]
}

@test "a_checksum_failure_leaves_the_install_dir_untouched" {
  mkdir -p "$HOME/foci-tune"
  printf 'old' >"$HOME/foci-tune/foci-tune-armv7"
  write_sums "$(printf '0%.0s' {1..64})"
  run foci_main
  [ "$status" -ne 0 ]
  [ "$(cat "$HOME/foci-tune/foci-tune-armv7")" = "old" ]
  [ ! -e "$HOME/foci-tune/release_info.json" ]
  ! grep -q '^install$' "$TEST_ROOT/calls.log"
  ! grep -q '^sudo:' "$TEST_ROOT/calls.log"
}

@test "a_failed_setup_leaves_files_in_place_and_prints_the_retry_command" {
  touch "$TEST_ROOT/sudo_fail"
  run foci_main
  [ "$status" -ne 0 ]
  [ -x "$HOME/foci-tune/foci-tune-armv7" ]
  [ -f "$HOME/foci-tune/release_info.json" ]
  [[ "$output" == *"sudo $HOME/foci-tune/foci-tune setup"* ]]
  ! grep -q '^block$' "$TEST_ROOT/calls.log"
  ! grep -q '^restart:' "$TEST_ROOT/calls.log"
}

@test "an_unmanaged_update_manager_foci_tune_section_is_refused_before_anything_is_touched" {
  printf '[update_manager foci-tune]\ntype: web\n' >>"$MOONRAKER_CONF"
  cp "$MOONRAKER_CONF" "$TEST_ROOT/conf.before"
  run foci_main
  [ "$status" -ne 0 ]
  [[ "$output" == *"update_manager foci-tune"* ]]
  [[ "$output" == *"$MOONRAKER_CONF"* ]]
  cmp -s "$MOONRAKER_CONF" "$TEST_ROOT/conf.before"
  ! grep -q -e '^sudo:' -e '^install$' -e '^download$' -e '^restart:' "$TEST_ROOT/calls.log"
  [ ! -e "$HOME/foci-tune" ]
}

@test "a_managed_block_from_a_previous_run_is_refreshed_not_refused" {
  foci_main
  sed -i.bak 's/channel: stable/channel: beta/' "$MOONRAKER_CONF" && rm -f "$MOONRAKER_CONF.bak"
  run foci_main
  [ "$status" -eq 0 ]
  grep -q 'channel: stable' "$MOONRAKER_CONF"
  ! grep -q 'channel: beta' "$MOONRAKER_CONF"
  [ "$(grep -c '^\[update_manager foci-tune\]' "$MOONRAKER_CONF")" -eq 1 ]
}

@test "a_rerun_does_not_restart_foci_tune_and_prints_the_restart_command" {
  foci_main
  printf 'binary-v2' >"$TEST_ROOT/fixture/foci-tune-armv7"
  write_sums "$(digest_of "$TEST_ROOT/fixture/foci-tune-armv7")"
  : >"$TEST_ROOT/calls.log"
  run foci_main
  [ "$status" -eq 0 ]
  [[ "$output" == *"sudo systemctl restart foci-tune"* ]]
  ! grep -q '^restart:foci-tune' "$TEST_ROOT/calls.log"
  ! grep -q 'systemctl' "$TEST_ROOT/calls.log"
  [ "$(cat "$HOME/foci-tune/foci-tune-armv7")" = "binary-v2" ]
}

@test "an_unchanged_rerun_does_not_print_the_restart_command" {
  foci_main
  run foci_main
  [ "$status" -eq 0 ]
  [[ "$output" != *"sudo systemctl restart foci-tune"* ]]
}

@test "running_as_root_is_refused" {
  require_not_root() { echo "must not be root" >&2; exit 1; }
  run foci_main
  [ "$status" -ne 0 ]
  [ -z "$(calls)" ]
}

@test "an_unsupported_architecture_fails_before_any_download" {
  uname() { echo armv6l; }
  run foci_main
  [ "$status" -ne 0 ]
  ! grep -q -e '^release_api$' -e '^download$' "$TEST_ROOT/calls.log"
}

@test "the_moonraker_block_has_the_spec_fields" {
  run foci_main
  [ "$status" -eq 0 ]
  local section
  section="$(awk '/^\[update_manager foci-tune\]/{f=1;next} /^\[|^# ---/{f=0} f' "$MOONRAKER_CONF")"
  [ "$section" = "type: executable
channel: stable
repo: foci-rs/foci-tune
path: ~/foci-tune
managed_services: foci-tune" ]
  grep -q '^host: 0.0.0.0$' "$MOONRAKER_CONF"
}

@test "help_exits_zero_without_touching_anything" {
  run foci_main --help
  [ "$status" -eq 0 ]
  [ -z "$(calls)" ]
}
