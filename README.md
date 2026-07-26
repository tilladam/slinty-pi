# slinty-pi

A native desktop app for the [pi coding agent](https://pi.dev), built with Rust and
[Slint](https://slint.dev). Local-first: designed around llama.cpp / Ollama / LM Studio, with
cloud providers through the same picker.

See [PRODUCT_PLAN.md](PRODUCT_PLAN.md) for the full product plan and milestones.

## Status

**M0 (skeleton & spike).** A minimal window that drives `pi --mode rpc`: streaming transcript
(text, thinking, tool activity), steering while the agent runs, abort, model display, and
token/cost stats.

## Crates

- `crates/pi-rpc` — typed client for pi's RPC mode: spawns `pi --mode rpc`, strict-LF JSONL
  framing, request/response correlation, tolerant event deserialization, extension-UI replies.
- `crates/slinty-pi` — the Slint app. Tokio backend owns the pi child process; all UI mutation
  goes through the Slint event loop with streaming deltas coalesced at ~33 ms.

## Running

Requires a [pi installation](https://pi.dev) (`pi` on PATH) with a configured model:

```sh
cargo run -p slinty-pi
```

### Demo mode (no pi needed)

Streams synthetic tokens — used as the M0 performance harness:

```sh
SLINTY_DEMO=1 cargo run -p slinty-pi
# optional: SLINTY_DEMO_RATE=500 (tokens/sec), SLINTY_DEMO_AUTOSEND="prompt" (stream on launch)
```

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

Run with `SLINT_MCP_PORT=<port>` set to enable the server, then talk JSON-RPC to
`http://127.0.0.1:<port>/mcp` (`tools/list` for the available tools — window/element
inspection, screenshots, click/drag/key simulation). Move these back to crates.io
version pins once both land in a release.

## Tests

```sh
cargo test
```

`pi-rpc` integration tests run against the real `pi` binary when present (offline, session-less,
extensions disabled) and skip otherwise.
