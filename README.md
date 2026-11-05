# foci-tune

Moonraker-attached tuning service for FOCI motion controllers. Analysis routines live in
`foci-tune-analysis` (pure, no I/O); the service binary is `foci-tune`.

## Build

```sh
rustup target add armv7-unknown-linux-musleabihf aarch64-unknown-linux-musl x86_64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl
```

Prerequisites: Node 22 (the web UI is built and embedded during `cargo build`) and a C compiler
for the target, because `ring` (via rustls) compiles C and assembly. Cross builds need the
matching cross compiler and a `CC_<target>` variable naming it:

```sh
sudo apt-get install -y gcc-aarch64-linux-gnu    # armv7: gcc-arm-linux-gnueabihf; x86_64: musl-tools
export CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc
```

`.github/workflows/ci.yml` shows the exact setup for all three targets.

### foci-trace

The `foci-tune` crate depends on [foci-trace](https://github.com/foci-rs/foci-trace) as a git
dependency, like the other FOCI components. `Cargo.lock` pins the exact revision; `cargo update -p
foci-trace` moves the pin to the tip of foci-trace's default branch.

To build against a local foci-trace checkout instead (such as `host/foci-trace` in the FOCI
aggregate checkout), add a source replacement to your own `~/.cargo/config.toml`:

```toml
[source.foci-trace-github]
git = "https://github.com/foci-rs/foci-trace"
replace-with = "foci-trace-local"

[source.foci-trace-local]
git = "file:///absolute/path/to/foci-trace"
```

Cargo then builds the revision pinned in `Cargo.lock` from that checkout and leaves the lock file
unchanged, so the pinned commit must exist there; uncommitted foci-trace changes are not seen. A
`[patch]` entry with a `path` would also work, but it rewrites the foci-trace entry in
`Cargo.lock` on every build. `cargo update -p foci-trace` refuses to run while the replacement is
active. To pin a newer local commit, comment out `replace-with` and point git at the checkout for
one update:

```sh
GIT_CONFIG_COUNT=1 \
GIT_CONFIG_KEY_0=url.file:///absolute/path/to/foci-trace.insteadOf \
GIT_CONFIG_VALUE_0=https://github.com/foci-rs/foci-trace \
cargo --config net.git-fetch-with-cli=true update -p foci-trace
```

The resulting lock entry is the same one an update against GitHub produces once that commit is
pushed.

## Run

```sh
foci-tune --moonraker-url http://127.0.0.1:7125 --listen 0.0.0.0:7130
foci-tune --check
```

### UI development

The UI hot-reloads under Vite without rebuilding the Rust binary. Start the backend once against a
simulated printer, then the dev server, which proxies `/api` (including the event socket) to it:

```sh
cargo run -p foci-tune -- --fake        # debug build only; moves nothing
cd web && npm run dev
```

`FOCI_TUNE_API` points the proxy elsewhere, for example at a real service. Buttons in the dev UI then
act on that printer, so use it for read-only screens.

```sh
FOCI_TUNE_API=http://printer.local:7130 npm run dev
```

## Install

The service drives printer motion. The UI requires the operator to start every run, and Kill is
the red button; keep the printer in view during a run.

On the Pi, as the normal user (not root):

```sh
curl -sL https://raw.githubusercontent.com/foci-rs/foci-tune/main/install.sh | bash
```

Add `-s -- --version <tag>` after `bash` to install a specific release instead of the latest, for
example `bash -s -- --version v0.2.0`. The installer needs Klipper and Moonraker already set up,
refuses to run while a print is active, and checks the download against the release's
`SHA256SUMS` before touching anything. It then:

- installs the binary into `~/foci-tune`,
- runs `sudo ~/foci-tune/foci-tune setup`, which installs the udev rule and the systemd unit,
  adds the service user to `plugdev`, registers the service in Moonraker's `moonraker.asvc` and
  enables it,
- adds an `[update_manager foci-tune]` section to `moonraker.conf` and restarts Moonraker.

An existing `[update_manager foci-tune]` section that the installer did not write makes it stop
without changing anything. Re-running the installer is safe and never restarts a running
`foci-tune`; if it replaced the binary, it tells you to run `sudo systemctl restart foci-tune`
once no run is active.

`~/foci-tune` holds the architecture-specific binary (`foci-tune-aarch64`, `foci-tune-armv7` or
`foci-tune-x86_64`), a `foci-tune` symlink to it, and `release_info.json`, which Moonraker's
update manager reads. The service runs from the symlink, so an update replaces the binary without
rewriting the unit.

### Updating

Update from Moonraker's update manager (Mainsail or Fluidd). Moonraker restarts the `foci-tune`
service after the update, including while a tuning run is in progress, which cuts that run off.
Update only when no run is active.

### Manual install

Release binaries are `foci-tune-aarch64`, `foci-tune-armv7` and `foci-tune-x86_64`, with a
`SHA256SUMS` file. To install without the installer, place the binary for the Pi's architecture
in `~/foci-tune`, create the `foci-tune` symlink to it, and run setup through the symlink:

```sh
sudo ~/foci-tune/foci-tune setup
```

`setup` installs the udev rule and the systemd unit, adds the service user to `plugdev`, adds
`foci-tune` to Moonraker's `moonraker.asvc` and enables the service. It is idempotent. It takes
the service user from `SUDO_USER` and refuses to run the service as root. The options are
described by `foci-tune setup --help`:

- `--dry-run` shows the plan and the full file contents and needs no root.
- `--remove` stops and disables the service and removes the unit, the udev rule and the
  `moonraker.asvc` entry. It keeps the user's `plugdev` membership, the session data, any
  `foci-tune.service.d` drop-in and `~/foci-tune`.
- `--user <name>` runs the service as another user.
- `--asvc-path <file>` names Moonraker's `moonraker.asvc` directly; otherwise setup asks Moonraker
  where it lives.
- `--moonraker-url <url>` is the Moonraker address used for that lookup (default
  `http://127.0.0.1:7125`).

Setup never restarts a running service, so it cannot cut off a tuning run. After replacing the
binary by hand, run `sudo systemctl restart foci-tune` when no run is active.

The unit passes the Moonraker URL and listen address as explicit flags in `ExecStart`. Setup
rewrites the unit on every run, so if Moonraker needs `--api-key`, add it in a drop-in:
`sudo systemctl edit foci-tune` and enter, with `<home>` replaced by the absolute path of the
service user's home directory:

```ini
[Service]
ExecStart=
ExecStart=<home>/foci-tune/foci-tune --moonraker-url http://127.0.0.1:7125 \
    --listen 0.0.0.0:7130 --api-key <key>
```

The empty `ExecStart=` clears the unit's command before the new one replaces it. The UI is on
port 7130 of the Pi. Sessions are stored in the `foci-results` directory inside Moonraker's
`config` root, so the service user must be able to write there. A group membership added by setup
applies to new logins.

`foci-tune --check` loads the embedded web assets and exits 0, or non-zero for a build without
them. The unit runs it before each start.

`foci-tune verify-archive --kind <probe|tracking|standstill> <capture>...` runs an analysis over
archived capture files (plain or zstd) and compares the result with the reference table, for
manual checks of analysis changes. Probe lines gate the velocity fit and the edge count; the
torque fit and torque PSD peak are printed for information only, since the reference scripts
measured no usable torque for them. Torque is the torque controller's target, the high half of
the TMC4671 `PID_TORQUE_TARGET_TORQUE_ACTUAL` register. Tracking lines gate the error standard deviation, p99 and cruise mean;
the reversal peak and count are printed beside the reference values for information only. The
reference scripts counted sign changes of the measured velocity, which encoder quantisation
miscounts, while foci-tune takes reversals from the commanded position.

## License

MIT OR Apache-2.0.
