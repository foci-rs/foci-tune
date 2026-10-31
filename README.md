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

Release binaries are named `foci-tune-<target>`; install the one for the Pi's architecture as
`foci-tune`.

```sh
sudo install -m 0755 foci-tune-aarch64-unknown-linux-musl /usr/local/bin/foci-tune
sudo cp 99-foci-trace.rules /etc/udev/rules.d/
sudo cp foci-tune.service /etc/systemd/system/
sudo usermod -aG plugdev pi    # if pi is not already in plugdev; log in again afterwards
sudo udevadm control --reload && sudo udevadm trigger
sudo systemctl daemon-reload
sudo systemctl enable --now foci-tune
```

The rule gives the `plugdev` group access to the FOCI trace USB device (vendor `1d50`, product
`614e`). The unit runs as `pi` after `moonraker.service`, with the Moonraker URL and listen
address as explicit flags in `ExecStart`; add `--api-key` there if Moonraker needs one. The UI
is on port 7130 of the Pi. Sessions are stored in the `foci-results` directory inside
Moonraker's `config` root, so `pi` must be able to write there.

`foci-tune --check` loads the embedded web assets and exits 0, or non-zero for a build without
them. The unit runs it before each start.

`foci-tune verify-archive --kind <probe|tracking|standstill> <capture>...` runs an analysis over
archived capture files (plain or zstd) and compares the result with the reference table, for
manual checks of analysis changes.

## License

MIT OR Apache-2.0.
