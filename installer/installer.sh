#!/usr/bin/env bash

foci_map_arch() {
  case "$1" in
    aarch64) echo aarch64 ;;
    armv7l) echo armv7 ;;
    x86_64) echo x86_64 ;;
    *)
      echo "Unsupported architecture '$1': foci-tune supports aarch64, armv7l and x86_64." >&2
      return 1
      ;;
  esac
}

_foci_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

foci_verify_checksum() {
  local sums_file="$1" asset="$2" file="$3"
  local lines expected actual
  lines="$(awk -v name="$asset" '$2 == name || $2 == "*" name { print $1 }' "$sums_file")" || return 1
  [ -n "$lines" ] || return 1
  [ "$(printf '%s\n' "$lines" | wc -l | tr -d ' ')" = "1" ] || return 1
  expected="$lines"
  actual="$(_foci_sha256 "$file")" || return 1
  [ "$expected" = "$actual" ]
}

_foci_json_escape() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  printf '%s' "$s"
}

foci_install_files() {
  local dir="$1" asset="$2" downloaded="$3" tag="$4" owner="$5" project="$6"
  mkdir -p "$dir" || return 1
  cp "$downloaded" "$dir/.$asset.new" || return 1
  chmod 0755 "$dir/.$asset.new" || return 1
  mv -f "$dir/.$asset.new" "$dir/$asset" || return 1
  ln -s "$asset" "$dir/.foci-tune.new" || return 1
  mv -fh "$dir/.foci-tune.new" "$dir/foci-tune" 2>/dev/null ||
    mv -fT "$dir/.foci-tune.new" "$dir/foci-tune" || return 1
  printf '{\n  "project_name": "%s",\n  "project_owner": "%s",\n  "version": "%s",\n  "asset_name": "%s"\n}\n' \
    "$(_foci_json_escape "$project")" "$(_foci_json_escape "$owner")" \
    "$(_foci_json_escape "$tag")" "$(_foci_json_escape "$asset")" >"$dir/release_info.json.new" || return 1
  mv -f "$dir/release_info.json.new" "$dir/release_info.json"
}
