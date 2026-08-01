# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

slinty-pi is a native desktop frontend for the [pi coding agent](https://pi.dev), built with Rust
and Slint. It drives `pi --mode rpc` as a subprocess (never forks pi's own logic) and is local-first:
designed around llama.cpp / Ollama / LM Studio, with cloud providers through the same picker.
`PRODUCT_PLAN.md` is the requirements baseline (vision, prior-art research on pi's RPC protocol,
milestones M0–M5); read it for product/architecture rationale before making non-trivial changes.

## Commands

```sh
cargo run -p slinty-pi                    # requires `pi` on PATH with a configured model
SLINTY_DEMO=1 cargo run -p slinty-pi      # demo mode: synthetic token stream, no pi needed
cargo test                                 # pi-rpc's integration tests skip if `pi` isn't installed
cargo test -p pi-rpc get_state_round_trips # run a single test
```

MCP (embedded Slint introspection/screenshot server) is opt-in per invocation, not a default
feature — see the "Slint dependency" section of README.md:

```sh
SLINT_EMIT_DEBUG_INFO=1 SLINT_MCP_PORT=9315 cargo run -p slinty-pi --features slint/mcp
```

Useful env vars for driving the UI without a display/accessibility automation (each is a
`"<delay_ms>:<arg>"` test hook wired in `main.rs`, mirroring a real UI action):
`SLINTY_SEND_AFTER`, `SLINTY_SWITCH_PROJECT_AFTER`, `SLINTY_SWITCH_SESSION_AFTER`,
`SLINTY_NEW_SESSION_AFTER`, `SLINTY_DELETE_SESSION_AFTER`, `SLINTY_SIDEBAR_SEARCH_AFTER`,
`SLINTY_OPEN_TREE_AFTER`, `SLINTY_FORK_FROM_AFTER`, `SLINTY_OPEN_PALETTE_AFTER`,
`SLINTY_PALETTE_QUERY_AFTER`, `SLINTY_PALETTE_EXEC_AFTER`, `SLINTY_ATTACH_AFTER`,
`SLINTY_CYCLE_DENSITY_AFTER`, `SLINTY_RESUME_SESSION`, `SLINTY_DEMO_RATE`, `SLINTY_DEMO_AUTOSEND`,
`SLINTY_DEMO_REPEATS`,
`SLINTY_OPEN_MODELS_AFTER`, `SLINTY_SERVE_RAPID_MLX_AFTER`, `SLINTY_LOAD_ROUTER_MODEL_AFTER`,
`SLINTY_UNLOAD_ROUTER_MODEL_AFTER`, `SLINTY_HF_SEARCH_AFTER`, `SLINTY_DOWNLOAD_HF_MODEL_AFTER`,
`SLINTY_ADD_OLLAMA_AFTER`.

In demo mode, a message starting with `md!` streams the rest of the message itself as the
assistant markdown — combined with `SLINTY_SEND_AFTER="800:md!…"` this drives arbitrary
rendering test cases through the real segmenter/highlighter pipeline (used for screenshot-based
rendering QA via the MCP server).

## Slint dependency: local checkout, not crates.io

`slint`, `slint-build`, and `i-slint-backend-winit` are **path dependencies** on a local Slint
checkout (master) at `/Users/till/Code/Rust/slint/slint`. This is required for the `mcp` feature
and for [PR #11520](https://github.com/slint-ui/slint/pull/11520) (`set_platform()` auto-starting
the MCP server for custom-platform apps). Move these back to crates.io pins once both land in a
release. Because `i-slint-backend-winit` is an internal, non-semver-stable crate, it must stay in
lockstep with whatever `slint` commit is checked out — if editing dependency versions, check that
checkout, don't just bump a version number.

## Architecture

Three crates:

- **`pi-rpc`** — typed async client for pi's RPC mode (`client.rs` spawns `pi --mode rpc` and
  frames strict-LF JSONL over stdin/stdout with request-id correlation; `types.rs` has the
  wire types). Deserialization is deliberately tolerant: unknown event/message kinds fall back to
  `Event::Unknown` / `AssistantMessageEvent::Unknown` rather than erroring, so newer pi versions
  don't break the client.
- **`pi-sessions`** — read-only index over pi's on-disk session JSONL tree
  (`~/.pi/agent/sessions/--<cwd>--/<ts>_<uuid>.jsonl`). pi remains the sole writer; this crate only
  ever reads, for the sidebar/session-tree UI.
- **`slinty-pi`** — the Slint app itself.

### Threading model (slinty-pi)

Slint owns the main thread and runs its own winit event loop; a tokio multi-thread runtime on
background threads owns the `pi` child process end-to-end. The two communicate one-way each
direction:

- UI → backend: Slint callbacks (`app.on_*` in `main.rs`) send a `backend::UiCmd` over an
  `mpsc::unbounded_channel`.
- backend → UI: every UI mutation goes through `Weak::upgrade_in_event_loop` (see `backend::Ui`).
  Closures run on the Slint thread in submission order, so the backend tracks a shadow row count
  to address transcript rows it appended without reading UI state back.
- Streaming text/thinking deltas are coalesced (`TEXT_FLUSH` = 33ms) before hitting
  `set_row_data`; tool-call partial results at `TOOL_FLUSH` = 100ms.

`main.rs` also installs an `i_slint_backend_winit::CustomApplicationHandler` (`DropFileHandler`)
*before* creating any window, because Slint's own winit backend never surfaces
`WindowEvent::DroppedFile` — this is the only way to see a real Finder/Explorer drag-and-drop; see
`attach.rs`'s doc comment.

`SLINTY_DEMO=1` swaps `backend::pi_backend` for `backend::demo_backend`, a synthetic token
streamer that exercises the same rendering path without spawning `pi` — used as the M0/perf
harness and for env-var-driven UI testing.

### Transcript rendering pipeline

`backend.rs` turns pi's JSON messages into `RowSpec`s (`hydrate_rowspecs` for session load,
incremental updates during streaming), which become Slint `Row` model entries. Markdown prose goes
through `segmenter.rs` (pulldown-cmark → `Segment`s) since Slint's `StyledText`/`@markdown` has no
fenced code blocks, headings, tables, or images — code segments get a custom component with
`syntect` highlighting (`highlight.rs`) instead of `StyledText`.

### Slint UI files (`ui/*.slint`)

`app.slint` is the main window and per-row-kind components (`UserRow`, `ProseRow`, `HeadingRow`,
`CodeRow`, `ThinkingRow`, `ToolRow`, `NoteRow`); `sidebar.slint`, `tree.slint`, `palette.slint` are
the session sidebar, branch-tree overlay, and command palette. `build.rs` compiles `ui/app.slint`
via `slint-build`; the other three are `import`ed from it (not standalone build targets).

Known Slint 1.17 layout constraints (see also the "Slint gotchas" project memory):

- Binding a child's `width`/`max-width` to an ancestor's width inside a `Flickable` creates a
  layoutinfo binding loop — use `ListView` instead of `Flickable` for anything width-dependent;
  cap bubble width with stretch-spacer `Rectangle`s (`horizontal-stretch: 1; min-width: Npx`)
  beside the content, not width bindings.
- Never size a `TouchArea` from a child's width (e.g. `width: chip.width`) — put the `TouchArea`
  *inside* the sized element instead, or it loops against the child's own fill-parent default.
- `StyledText` rows don't appear in the macOS accessibility tree; verify prose visually (or via the
  MCP screenshot/element-tree tools) rather than `osascript`.

## Extending the RPC protocol surface

`pi-rpc`'s `Command`/`Event`/`AssistantMessageEvent` enums are the sanctioned contract with pi
(`docs/rpc.md` in pi-coding-agent). When pi adds new commands or event kinds, add the matching
serde variant rather than parsing raw `Value` in `backend.rs` — the tolerant `#[serde(other)]`
fallback is there so *unrecognized* protocol additions don't crash the client, not as a substitute
for typing the ones this app actually uses.
