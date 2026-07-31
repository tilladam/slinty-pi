# slinty-pi

A native desktop app for the [pi coding agent](https://pi.dev), built with Rust and
[Slint](https://slint.dev). Local-first: designed around llama.cpp / Ollama / LM Studio, with
cloud providers through the same picker.

See [PRODUCT_PLAN.md](PRODUCT_PLAN.md) for the full product plan and milestones, and
[CLAUDE.md](CLAUDE.md) for architecture notes and the full list of env-var test hooks.

## Status

**M0–M2 done, M3 (local models) well underway.** Streaming transcript (markdown, thinking blocks,
collapsible tool-call chips), composer with attach/steer/abort, session sidebar with search/
fork/clone/rename, a read-only branch-tree overlay, command palette, transcript density modes,
and a models panel covering rapid-mlx, the llama.cpp router (load/unload, Hugging Face search
and download), and a guided Ollama models.json editor. M4 (extension-UI protocol, permission
gating, diff viewer, compaction UX) hasn't started yet. A first slice of M5 has landed ahead of
the rest of that milestone — an app icon and `cargo-bundle` packaging (see below) — but signing,
notarization, and the CI performance gate are still open.

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
notarization, and stapling — see M5 in `PRODUCT_PLAN.md`.

## Slint dependency

`slint`, `i-slint-backend-winit`, and `slint-build` are path dependencies on a local
`slint` checkout (master) at `/Users/till/Code/Rust/slint/slint`, not crates.io. This is
needed for two things not yet in a published release (1.17.1 predates both):

- the `mcp` feature (embedded MCP server for UI introspection/screenshots — see that
  repo's `docs/development/mcp-server.md`)
- [PR #11520](https://github.com/slint-ui/slint/pull/11520), which makes
  `slint::platform::set_platform()` start that server automatically for custom-platform
  apps like this one (`main.rs` installs a `CustomApplicationHandler` to see winit's
  `WindowEvent::DroppedFile`, so it can't go through the default backend selector)

`mcp` is not enabled by default (it pulls in a whole extra dependency tree — async-net,
httparse, prost, protox, ...). Enable it per-invocation instead:

```sh
SLINT_EMIT_DEBUG_INFO=1 SLINT_MCP_PORT=9315 cargo run -p slinty-pi --features slint/mcp
```

`SLINT_EMIT_DEBUG_INFO=1` preserves element IDs/source locations, needed for full
element introspection. Then talk JSON-RPC to `http://127.0.0.1:9315/mcp` (`tools/list`
for the available tools — window/element inspection, screenshots, click/drag/key
simulation). See the [official Slint AI-plugins skill](https://github.com/slint-ui/ai-plugins)
(installed as a Claude Code plugin: `slint@slint`) for full usage details. Move these
back to crates.io version pins once `mcp` and PR #11520 land in a release.

## Tests

```sh
cargo test
```

`pi-rpc` integration tests run against the real `pi` binary when present (offline, session-less,
extensions disabled) and skip otherwise.
