# foci-tune

Moonraker-attached tuning service for FOCI motion controllers. Analysis routines live in
`foci-tune-analysis` (pure, no I/O); the service binary is `foci-tune`.

## Build

```sh
rustup target add armv7-unknown-linux-musleabihf aarch64-unknown-linux-musl x86_64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl
```

Pure-Rust dependencies only; musl targets link with `rust-lld`.

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

## License

MIT OR Apache-2.0.
