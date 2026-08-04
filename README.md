# slinty-pi

A native desktop app for the [pi coding agent](https://pi.dev), built with Rust and
[Slint](https://slint.dev). Local-first: designed around [rapid-mlx](https://rapidmlx.com)
(Apple Silicon) and llama.cpp's router, with Ollama detection/one-click setup and cloud
providers through the same picker.

See [CLAUDE.md](CLAUDE.md) for architecture notes and the full list of env-var test hooks.

## Crates

- `crates/pi-rpc` — typed client for pi's RPC mode: spawns `pi --mode rpc`, strict-LF JSONL
  framing, request/response correlation, tolerant event deserialization, extension-UI replies.
- `crates/pi-sessions` — read-only index over pi's on-disk session JSONL tree, used for the
  sidebar and session-tree UI. pi remains the sole writer; this crate only ever reads.
- `crates/slinty-pi` — the Slint app. Tokio backend owns the pi child process; all UI mutation
  goes through the Slint event loop with streaming deltas coalesced at ~33 ms.

## Running

Requires a [pi installation](https://pi.dev) (`pi` on PATH) with a configured model:

```sh
cargo run -p slinty-pi
```

### Demo mode (no pi needed)

Streams synthetic tokens instead of a real `pi` process — originally the M0 performance harness,
now also how most UI flows get driven headlessly for testing (see `SLINTY_*_AFTER` in CLAUDE.md):

```sh
SLINTY_DEMO=1 cargo run -p slinty-pi
# optional: SLINTY_DEMO_RATE=500 (tokens/sec), SLINTY_DEMO_AUTOSEND="prompt" (stream on launch)
```

## Packaging

[`cargo-bundle`](https://github.com/burtonageo/cargo-bundle) (`cargo install cargo-bundle`) reads
`[package.metadata.bundle]` in `crates/slinty-pi/Cargo.toml` to produce a macOS `.app` + `.dmg`:

```sh
cd crates/slinty-pi   # cargo-bundle resolves `icon` paths relative to the CWD it's run from,
                      # not the crate's own Cargo.toml — cd in first in this workspace
cargo bundle
open target/debug/bundle/osx/slinty-pi.app
```

The app icon lives in `crates/slinty-pi/assets/icon/` (`icon.icns` / `icon.ico` / `icon.png`, all
derived from one 1024px vector master); `icon.ico` is also embedded directly into the Windows
`.exe` via `embed-resource` in `build.rs`, independent of `cargo-bundle`. Not done yet: signing,
notarization, and stapling.

## Tests

```sh
cargo test
```

`pi-rpc` integration tests run against the real `pi` binary when present (offline, session-less,
extensions disabled) and skip otherwise.
