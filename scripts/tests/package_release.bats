#!/usr/bin/env bats

SCRIPT="${BATS_TEST_DIRNAME}/../package-release.sh"

setup() {
  ARTIFACTS="${BATS_TEST_TMPDIR}/artifacts"
  OUT="${BATS_TEST_TMPDIR}/out"
  for target in aarch64-unknown-linux-musl armv7-unknown-linux-musleabihf x86_64-unknown-linux-musl; do
    mkdir -p "${ARTIFACTS}/foci-tune-${target}"
    printf 'binary for %s\n' "$target" > "${ARTIFACTS}/foci-tune-${target}/foci-tune"
  done
}

verify_sums() {
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$OUT" && sha256sum -c SHA256SUMS)
  else
    (cd "$OUT" && shasum -a 256 -c SHA256SUMS)
  fi
}

@test "maps_each_rust_target_to_its_short_asset_name" {
  run "$SCRIPT" "$ARTIFACTS" "$OUT"
  [ "$status" -eq 0 ]
  [ "$(cat "$OUT/foci-tune-aarch64")" = "binary for aarch64-unknown-linux-musl" ]
  [ "$(cat "$OUT/foci-tune-armv7")" = "binary for armv7-unknown-linux-musleabihf" ]
  [ "$(cat "$OUT/foci-tune-x86_64")" = "binary for x86_64-unknown-linux-musl" ]
}

@test "writes_a_sha256sums_that_verifies_with_sha256sum_c" {
  run "$SCRIPT" "$ARTIFACTS" "$OUT"
  [ "$status" -eq 0 ]
  names="$(awk '{print $2}' "$OUT/SHA256SUMS" | tr '\n' ' ')"
  [ "$names" = "foci-tune-aarch64 foci-tune-armv7 foci-tune-x86_64 " ]
  grep -Eq '^[0-9a-f]{64}  foci-tune-' "$OUT/SHA256SUMS"
  run verify_sums
  [ "$status" -eq 0 ]
}

@test "fails_when_a_target_artifact_is_missing_and_writes_nothing" {
  rm "${ARTIFACTS}/foci-tune-armv7-unknown-linux-musleabihf/foci-tune"
  run "$SCRIPT" "$ARTIFACTS" "$OUT"
  [ "$status" -ne 0 ]
  [ ! -e "$OUT" ] || [ -z "$(ls -A "$OUT")" ]
}
