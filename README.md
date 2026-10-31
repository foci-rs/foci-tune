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

## Install

The service drives printer motion. The UI requires the operator to start every run, and Kill is
the red button; keep the printer in view during a run.

Release binaries are named `foci-tune-<target>`; download the one for the Pi's architecture and
let it install itself:

```sh
chmod +x foci-tune-aarch64-unknown-linux-musl
sudo ./foci-tune-aarch64-unknown-linux-musl setup
```

`setup` installs the binary, the udev rule and the systemd unit, adds the service user to
`plugdev` and enables the service. It is idempotent. The options (`--user`, `--dry-run`,
`--remove`) are described by `foci-tune setup --help`; `--dry-run` shows the plan and the full
file contents and needs no root. `--remove` keeps the user's `plugdev` membership, the session
data and any `foci-tune.service.d` drop-in.

Setup never restarts a running service, so it cannot cut off a tuning run. After upgrading, run
`sudo systemctl restart foci-tune` when no run is active.

The unit passes the Moonraker URL and listen address as explicit flags in `ExecStart`. Setup
rewrites the unit on every run, so if Moonraker needs `--api-key`, add it in a drop-in:
`sudo systemctl edit foci-tune` and enter

```ini
[Service]
ExecStart=
ExecStart=/usr/local/bin/foci-tune --moonraker-url http://127.0.0.1:7125 \
    --listen 0.0.0.0:7130 --api-key <key>
```

The empty `ExecStart=` clears the unit's command before the new one replaces it. The UI is on
port 7130 of the Pi. Sessions are stored in the
`foci-results` directory inside Moonraker's `config` root, so the service user must be able to
write there. A group membership added by setup applies to new logins.

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
