setup() {
  TEST_ROOT="$(mktemp -d "${BATS_TMPDIR:-/tmp}/foci-tune-helpers.XXXXXX")"
  log() { :; }
  export -f log
  source "$BATS_TEST_DIRNAME/../installer.sh"
}
teardown() { rm -rf "$TEST_ROOT"; }

digest_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

@test "maps_aarch64_armv7l_and_x86_64" {
  run foci_map_arch aarch64
  [ "$status" -eq 0 ]
  [ "$output" = "aarch64" ]
  run foci_map_arch armv7l
  [ "$status" -eq 0 ]
  [ "$output" = "armv7" ]
  run foci_map_arch x86_64
  [ "$status" -eq 0 ]
  [ "$output" = "x86_64" ]
}

@test "rejects_armv6l_and_unknown_arches_with_a_message" {
  for arch in armv6l riscv64 ""; do
    run foci_map_arch "$arch"
    [ "$status" -eq 1 ]
    [ -n "$output" ]
  done
}

@test "checksum_passes_on_a_matching_line" {
  printf 'payload' > "$TEST_ROOT/foci-tune-armv7"
  printf '%s  foci-tune-armv7\n%s  foci-tune-x86_64\n' "$(digest_of "$TEST_ROOT/foci-tune-armv7")" \
    "$(printf 'x%.0s' {1..64})" > "$TEST_ROOT/SHA256SUMS"
  run foci_verify_checksum "$TEST_ROOT/SHA256SUMS" foci-tune-armv7 "$TEST_ROOT/foci-tune-armv7"
  [ "$status" -eq 0 ]
}

@test "checksum_fails_on_a_mismatched_digest" {
  printf 'payload' > "$TEST_ROOT/foci-tune-armv7"
  printf '%s  foci-tune-armv7\n' "$(printf '0%.0s' {1..64})" > "$TEST_ROOT/SHA256SUMS"
  run foci_verify_checksum "$TEST_ROOT/SHA256SUMS" foci-tune-armv7 "$TEST_ROOT/foci-tune-armv7"
  [ "$status" -ne 0 ]
}

@test "checksum_fails_when_the_asset_has_no_line" {
  printf 'payload' > "$TEST_ROOT/foci-tune-armv7"
  printf '%s  foci-tune-x86_64\n' "$(digest_of "$TEST_ROOT/foci-tune-armv7")" > "$TEST_ROOT/SHA256SUMS"
  run foci_verify_checksum "$TEST_ROOT/SHA256SUMS" foci-tune-armv7 "$TEST_ROOT/foci-tune-armv7"
  [ "$status" -ne 0 ]
}

@test "checksum_fails_on_a_duplicated_line" {
  printf 'payload' > "$TEST_ROOT/foci-tune-armv7"
  d="$(digest_of "$TEST_ROOT/foci-tune-armv7")"
  printf '%s  foci-tune-armv7\n%s  foci-tune-armv7\n' "$d" "$d" > "$TEST_ROOT/SHA256SUMS"
  run foci_verify_checksum "$TEST_ROOT/SHA256SUMS" foci-tune-armv7 "$TEST_ROOT/foci-tune-armv7"
  [ "$status" -ne 0 ]
}

@test "install_files_creates_binary_symlink_and_release_info_with_the_exact_tag" {
  printf 'binary' > "$TEST_ROOT/dl"
  run foci_install_files "$TEST_ROOT/dest" foci-tune-armv7 "$TEST_ROOT/dl" v1.2.3-rc.1 foci-rs foci-tune
  [ "$status" -eq 0 ]
  [ -x "$TEST_ROOT/dest/foci-tune-armv7" ]
  [ "$(cat "$TEST_ROOT/dest/foci-tune-armv7")" = "binary" ]
  [ "$(readlink "$TEST_ROOT/dest/foci-tune")" = "foci-tune-armv7" ]
  [ "$(cat "$TEST_ROOT/dest/foci-tune")" = "binary" ]
  grep -q '"project_name": *"foci-tune"' "$TEST_ROOT/dest/release_info.json"
  grep -q '"project_owner": *"foci-rs"' "$TEST_ROOT/dest/release_info.json"
  grep -q '"version": *"v1.2.3-rc.1"' "$TEST_ROOT/dest/release_info.json"
  grep -q '"asset_name": *"foci-tune-armv7"' "$TEST_ROOT/dest/release_info.json"
}

@test "install_files_replaces_an_older_arch_binary_and_symlink" {
  printf 'old' > "$TEST_ROOT/dl-old"
  printf 'new' > "$TEST_ROOT/dl-new"
  foci_install_files "$TEST_ROOT/dest" foci-tune-armv7 "$TEST_ROOT/dl-old" v1.0.0 foci-rs foci-tune
  run foci_install_files "$TEST_ROOT/dest" foci-tune-aarch64 "$TEST_ROOT/dl-new" v1.1.0 foci-rs foci-tune
  [ "$status" -eq 0 ]
  [ "$(cat "$TEST_ROOT/dest/foci-tune")" = "new" ]
  [ -x "$TEST_ROOT/dest/foci-tune-aarch64" ]
  grep -q '"version": *"v1.1.0"' "$TEST_ROOT/dest/release_info.json"
  grep -q '"asset_name": *"foci-tune-aarch64"' "$TEST_ROOT/dest/release_info.json"
}
