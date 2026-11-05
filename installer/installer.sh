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
  ln -sfn "$asset" "$dir/.foci-tune.new" || return 1
  mv -fh "$dir/.foci-tune.new" "$dir/foci-tune" 2>/dev/null ||
    mv -fT "$dir/.foci-tune.new" "$dir/foci-tune" || return 1
  printf '{\n  "project_name": "%s",\n  "project_owner": "%s",\n  "version": "%s",\n  "asset_name": "%s"\n}\n' \
    "$(_foci_json_escape "$project")" "$(_foci_json_escape "$owner")" \
    "$(_foci_json_escape "$tag")" "$(_foci_json_escape "$asset")" >"$dir/release_info.json.new" || return 1
  mv -f "$dir/release_info.json.new" "$dir/release_info.json"
}

FOCI_RELEASE_API="${FOCI_RELEASE_API:-https://api.github.com/repos/foci-rs/foci-tune/releases/latest}"
FOCI_DOWNLOAD_BASE="${FOCI_DOWNLOAD_BASE:-https://github.com/foci-rs/foci-tune/releases/download}"
FOCI_OWNER="foci-rs"
FOCI_PROJECT="foci-tune"
FOCI_BLOCK_NAME="foci-tune"

foci_usage() {
  cat <<'USAGE'
Usage: foci-tune-installer.sh [--help]

Installs the latest foci-tune release into ~/foci-tune, runs `sudo foci-tune setup`,
and registers foci-tune with Moonraker's update manager. Run as the normal user.
USAGE
}

_foci_unmanaged_section() {
  local conf="$1"
  [ -f "$conf" ] || return 1
  awk -v start="# --- $FOCI_BLOCK_NAME ---" -v end="# --- /$FOCI_BLOCK_NAME ---" '
    $0 == start { in_block = 1; next }
    $0 == end { in_block = 0; next }
    !in_block && $0 ~ /^\[update_manager[ \t]+foci-tune\][ \t]*$/ { found = 1 }
    END { exit found ? 0 : 1 }
  ' "$conf"
}

_foci_require_idle() {
  if ! check_no_active_print; then
    log "A print is active or the printer state could not be determined; re-run when idle." >&2
    return 1
  fi
}

_foci_download() {
  local tmp="$1" asset="$2" body
  body="$(curl -fsSL "$FOCI_RELEASE_API")" || { log "Could not query the latest release." >&2; return 1; }
  FOCI_TAG="$(printf '%s' "$body" | json_get tag_name)" || FOCI_TAG=""
  if [ -z "$FOCI_TAG" ]; then
    log "The release API response had no tag_name." >&2
    return 1
  fi
  curl -fsSL -o "$tmp/$asset" "$FOCI_DOWNLOAD_BASE/$FOCI_TAG/$asset" ||
    { log "Download failed for $asset ($FOCI_TAG)." >&2; return 1; }
  curl -fsSL -o "$tmp/SHA256SUMS" "$FOCI_DOWNLOAD_BASE/$FOCI_TAG/SHA256SUMS" ||
    { log "Download failed for SHA256SUMS ($FOCI_TAG)." >&2; return 1; }
  if ! foci_verify_checksum "$tmp/SHA256SUMS" "$asset" "$tmp/$asset"; then
    log "Checksum verification failed for $asset; nothing was installed." >&2
    return 1
  fi
}

foci_main() {
  case "${1:-}" in
    "") ;;
    -h | --help) foci_usage; return 0 ;;
    *) log "Unknown argument: $1" >&2; return 1 ;;
  esac

  discover_klipper_env
  require_not_root

  if _foci_unmanaged_section "$MOONRAKER_CONFIG"; then
    log "$MOONRAKER_CONFIG already has an [update_manager foci-tune] section that this" >&2
    log "installer does not manage. Remove or rename it, then re-run; nothing was changed." >&2
    return 1
  fi

  _foci_require_idle || return 1

  local arch asset dir="$HOME/foci-tune" tmp
  arch="$(foci_map_arch "$(uname -m)")" || return 1
  asset="foci-tune-$arch"

  tmp="$(mktemp -d)" || return 1
  if ! _foci_download "$tmp" "$asset"; then
    rm -rf "$tmp"
    return 1
  fi

  local replaced=0
  if [ -f "$dir/$asset" ] && ! cmp -s "$dir/$asset" "$tmp/$asset"; then
    replaced=1
  fi
  if ! foci_install_files "$dir" "$asset" "$tmp/$asset" "$FOCI_TAG" "$FOCI_OWNER" "$FOCI_PROJECT"; then
    rm -rf "$tmp"
    log "Could not install files into $dir." >&2
    return 1
  fi
  rm -rf "$tmp"

  _foci_require_idle || return 1

  if ! sudo "$dir/foci-tune" setup; then
    log "foci-tune setup failed. The files are in place; retry with:" >&2
    log "  sudo $dir/foci-tune setup" >&2
    return 1
  fi

  local block
  block="type: executable
channel: stable
repo: foci-rs/foci-tune
path: ~/foci-tune
managed_services: foci-tune"
  # The subshell keeps kpi's die() from ending the installer; it inherits the caller's EXIT
  # trap, which kpi's atomic file writer would re-arm and fire a second time.
  if ! (
    trap - EXIT
    config_block_ensure "$MOONRAKER_CONFIG" "$FOCI_BLOCK_NAME" "[update_manager foci-tune]
$block"
  ); then
    log "Could not write the update manager block. foci-tune itself is installed." >&2
    log "Add this to $MOONRAKER_CONFIG and restart Moonraker:" >&2
    log "[update_manager foci-tune]" >&2
    log "$block" >&2
    return 1
  fi

  _foci_require_idle || {
    log "Restart Moonraker when idle: sudo systemctl restart moonraker" >&2
    return 1
  }
  if ! (trap - EXIT; service_restart_if moonraker); then
    log "Restart Moonraker manually: sudo systemctl restart moonraker" >&2
    return 1
  fi

  if [ "$replaced" -eq 1 ]; then
    log "The foci-tune binary was replaced. It takes effect after you run:" >&2
    log "  sudo systemctl restart foci-tune" >&2
  fi
  log "foci-tune is available at http://$(hostname):7130" >&2
}
