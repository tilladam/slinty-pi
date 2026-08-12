# SwiftUI branch: shared Rust core + native macOS app

> **Renamed (commit `415dbc7`)**: the Swift app now lives at `macos/swifty-pi/` (was
> `apps/pi-mac/`), the Xcode project/target/scheme/product is `SwiftyPi`/`SwiftyPi.app` (was
> `PiMac`/`PiMac.app`), and the bundle id / logging subsystem / `UserDefaults` key prefix / demo
> env var are `dev.slinty-pi.swifty-pi` / `SWIFTY_PI_DEMO` (were `dev.slinty-pi.pi-mac` /
> `PI_MAC_DEMO`). Every mention of `apps/pi-mac`, `PiMac`, or `pi-mac` below this line is
> historical — accurate as of when each section was written, not the current paths/names. New
> sections should use the current names.

## Context

CLAUDE.md's "Branch goals" section (added on this `swiftui` branch, not yet committed) states the
goal: build a second, macOS-only native UI using Swift/SwiftUI instead of Slint, ideally sharing
the Rust logic that drives `pi` between both apps. Today's `slinty-pi` is a single Slint app; a
code audit of `crates/slinty-pi/src/backend.rs` (4240 lines) shows it already has a clean internal
seam — only ~150-160 lines (a `Ui` struct + two small Slint-model-building helpers) actually touch
Slint types. Everything else (the pi-RPC-driven state machine, session listing, local-model
orchestration, markdown segmentation, syntax highlighting) is plain, Slint-free Rust. This plan
extracts that seam into a new shared crate first, proves it against the existing (working, tested)
Slint app, and only then builds a thin SwiftUI spike on top of it — mirroring how this project's
own `PRODUCT_PLAN.md` sequenced its M0 milestone ("prove the architecture end-to-end" before real
feature work).

All milestones land on the current `swiftui` branch. FFI mechanism: **UniFFI**. SwiftUI deployment
target: **macOS 14+** (enables `@Observable`).

**Status: SW0, SW1, SW2, SW3, and SW4 are implemented and committed, plus the app diagnostics fix**
(36 commits total, `cargo test --workspace` green throughout, and real `pi --mode rpc`/rapid-mlx/
Hugging Face round-trips plus a clean `xcodebuild -scheme PiMac build` all verified against this
real machine). SW3 reused
pi-core's Rust segmenter/highlighter/hydration pipeline over FFI (`crates/pi-render`); the Swift
app renders rich `RowRecord`-based transcripts and supports restoring/resuming sessions. SW4
extracted `crates/pi-local` (rapid-mlx/router/HF/Ollama/auth, shared by both frontends) and added a
browse/manage-only Models panel to Swift (rapid-mlx serve, router load/unload, HF search/download,
Ollama bulk-add, API key entry) — deliberately without a composer model picker or the server-dot
health indicator, both explicit follow-ups once a picker exists. **The app diagnostics fix is
implemented and committed** (3 commits: `tracing-oslog` wired into `pi-core-ffi` via a
`ensure_logging_initialized` `Once`-guarded install, the two previously-silent spawn-failure sites
plus `login_shell_path` logged, a `report_error` helper centralizing all 7 `ChatSink::on_error`
call sites, and a Swift-side `AppModel.setStatus` wrapping `os.Logger` around every one of the 15
`statusMessage` assignments) — real end-to-end verified: a forced spawn failure showed up in the
unified log archive (`log show`) as a red **Fault** entry tagged `[dev.slinty-pi.pi-mac:rust]` with
the full specific error message, and a clean `xcodebuild` confirmed no linking regression. **The
live-streaming markdown/code rendering fix is implemented and committed** (2 commits: a stateless
`PiSession.previewRows` FFI method reusing `pi-render`'s existing segmenter, no changes to
`run()`/`apply()`/`ChatSink`, plus `AppModel`'s throttled ~33ms `scheduleStreamPreview`/
`refreshStreamPreview` calling it and `ChatView` rendering `streamingRows` through the same
`RowView` family as finalized rows) — `cargo test --workspace` green (new `preview_rows` unit
tests included), a clean `xcodebuild` succeeded, and real end-to-end use confirmed it renders
markdown/code richly while a reply is still streaming. **SW5 (extension-UI dialogs & permission
gating) is implemented and committed** (2 commits: `pi-core-ffi` gained `ExtensionDialogRecord`/
`ExtensionDialogReply`, a `ChatSink::on_extension_dialog` callback fired from `apply()` for the
four blocking dialog kinds, and a fire-and-forget `PiSession.reply_extension_dialog` wrapping
`PiClient::reply_extension_ui`; `AppModel` gained a FIFO `pendingDialogs` queue and a new
`ExtensionDialogView` renders `confirm`/`input` via `.alert` and `select`/`editor` via `.sheet`) —
`cargo test --workspace` green (new `dialog.rs` conversion + `apply()` routing tests), a clean
`xcodebuild` succeeded, and the new `spike_check.rs` `dialogs` mode round-tripped a real dialog
end-to-end against this machine's already-installed `pi-permission-system` extension (which turned
out to use `method=select`, not the hypothetical `confirm` the aspirational M4 doc suggested — the
generic, not-title-special-cased rendering design handled this correctly), and a manual in-app
click-through (run by the user, since this sandboxed environment can't launch/interact with the
GUI itself) confirmed the real dialog renders and resolves correctly end-to-end. **SW6 (composer
model picker + server-dot health indicator) is implemented and committed** (2 commits: `pi-core-ffi`
gained a new `models.rs` porting `pi-core`'s `refresh_models`/`model_label`/`compute_server_dot`/
`classify_rapid_mlx_dot`/`probe_tcp`, `PiSession.refresh_models`/`set_model` (pull-based, mirroring
`SessionIndex`/`LocalModelIndex`), and a 5s-polled `dot_interval` inside `run()` pushing
`ChatSink.on_server_dot_changed` only on change; `AppModel`/`ChatView` gained a composer `Menu`
picker and a status-bar health dot mirroring `app.slint`'s green/red/amber semantics) — `cargo test
--workspace` green (new `models.rs` label-formatting + `classify_rapid_mlx_dot` truth-table tests),
a clean `xcodebuild` succeeded, and the extended `spike_check.rs` `models` mode round-tripped
`refresh_models`/`set_model` against this machine's real `pi` config: 19 real models listed with
correctly-formatted labels, the actual currently-active local rapid-mlx model correctly flagged,
and `set_model` succeeded with no hang. In-app GUI click-through found and fixed two real bugs
(`ForEach` keyed on a non-unique model `id` — pi can list the same id under multiple providers —
and `Label`'s SF Symbol icon not rendering inside a `Menu` on this machine's beta macOS/Xcode SDK;
fixed by keying on array offset and a plain-text `✓` prefix respectively), confirmed working by the
user. **SW7 (live thinking/tool-call visibility) is implemented and committed** (2 commits:
`pi-core-ffi` gained a new `live_preview.rs` porting `pi-core`'s `ThinkingRegion`/`ToolRun` live
state machine (33ms/100ms throttled flushes, `tool_summary`/`tail` formatting reused from
`pi-render`) and two new keyed `ChatSink` callbacks — `on_thinking_row_changed`/
`on_tool_row_changed` — pushed from a new `apply_live_preview` call inside `run()`'s event loop,
alongside (not replacing) `apply()`; `AppModel`/`ChatView` gained `liveThinkingRows`/`liveToolRows`
arrays rendered via `RowView`'s existing `ThinkingRowView`/`ToolRowView` unchanged) — `cargo test
--workspace` green (new `live_preview.rs` unit tests, including a `format_elapsed` truth table and
concurrent-tool-call isolation), a clean `xcodebuild` succeeded, and the new `spike_check.rs`
`live` mode proved the whole pipeline end-to-end against a real `pi` tool-using turn: a live
thinking row streamed incrementally and finalized, a second thinking block in the same turn
correctly got a fresh id, and a tool row showed `running=true` (`⚙ read Cargo.toml`) *while the
turn was still in flight*, finalizing to `running=false` with a `✓` mark and real elapsed time —
exactly the previously-nonexistent live visibility this milestone set out to add. In-app SwiftUI
click-through was confirmed working by the user, who also asked for two small status-bar
follow-ups (their own commits, not part of SW7 proper): tooltips on both color dots — dropping the
now-redundant "streaming…"/"idle" text label in favor of a hover tooltip, and adding one to the
server-health dot explaining its color — and a hit-area fix (`Circle()` only hit-tests its filled
path by default, too small at 8pt for reliable hover; widened via padding + `contentShape`).
**The "Native NSPasteboard copy" fix is implemented and committed** (1 commit, pure Swift — no
`pi-core-ffi` changes needed, `RowRecord` already carried everything required): a new `CopyButton`
view in `RowView.swift` mirroring `app.slint`'s component (click → copy → "✓" for 1.4s), wired
into `CodeBlockView` (always shown, copies `row.text`) and into `RowView`'s `prose`/`quote`/
`heading` cases via a `withGroupCopy` helper (shown once per message group when `row.first &&
!row.raw.isEmpty`, copies `row.raw`) — clean `swiftc -typecheck` and `xcodebuild`. In-app manual
click-through (paste-and-check the clipboard contents) still needs a pass by the user.

**Six more items are planned in detail below**, all requested together in one round and ordered
from smallest to largest. **The first — user message bubble styling + delete-session
confirmation — is implemented and committed** (1 commit, pure Swift, zero `pi-core-ffi` changes:
`RowView`'s `"user"` case now renders right-aligned in a `Color.accentColor.opacity(0.12)`-tinted
rounded bubble mirroring `app.slint`'s `UserRow`; `SidebarView`'s delete action now confirms via a
native `.alert` noting Trash-recoverability instead of firing immediately) — clean `xcodebuild`.
**The transcript density control (Verbose/Normal/Summary) is implemented and committed** (commit
`7927d77`, pure Swift, zero `pi-core-ffi` changes: `AppModel` gained a persisted `density`
(0/1/2) + `setDensity`/`rowVisible`, mirroring `app.slint`'s `cycle-density`/`Style.row-visible`
semantics — Summary hides `thinking`/`tool`/`info` rows, live ones included, errors always show;
`ChatView` filters all four row-rendering loops through `rowVisible`, force-expands tool rows in
Verbose, and — per user follow-up request — surfaces the mode via a flat toolbar `Picker` (not the
originally-planned cycle button) so all three levels are directly selectable, opted out of the
macOS 26+ glass background the same way the status dots are) — clean `xcodebuild`, confirmed
working by the user.

Also implemented and committed, requested directly (not part of the original six-item batch):
**current-project visibility + File-menu previous-projects switcher** (commit `f01232d`:
`ChatView`'s window title/subtitle now show the current project's name and full path, replacing
the hardcoded `"pi"`/`"New Session"` text; a new File → Open Recent Project menu lists other known
projects via `SessionIndex.listProjects` — already fetched but previously unused — and Open
Project… (⌘O) opens the same folder picker as the sidebar's toolbar button; `AppModel` moved up to
be owned by `SwiftyPiApp` so the File-menu commands can reach it) — clean `xcodebuild`, confirmed
working by the user.

**Milestone SW8 (thinking-level control + session stats indicator) is implemented and committed**
(commit `8784840`: `pi-core-ffi` gained `thinking.rs` — `ThinkingLevelKind`/`ThinkingLevelRecord`
porting `refresh_thinking`, `SessionStatsRecord` porting `update_stats`, both wired through new
`ChatCmd`/`PiSession`/`ChatSink` surface mirroring SW6's model-picker shape exactly; the stats push
lands from inside `hydrate_and_push`, which already covers every trigger point in one place — real
`pi` verification via `spike_check --thinking` confirmed 5 real levels with the correct current
flag, a clean switch, and a real stats push after a turn settled, also resolving the plan's one
open question: `contextUsage.percent` is 0-100, not a 0-1 fraction) — plus follow-up polish
requested directly after the first pass: the thinking-level picker sits side by side with the
model picker in the sidebar footer (labels shortened, no more `"think: "` prefix), and the chat
pane's usage ring got a visible track color for the unused portion and moved (with the rest of the
status bar) below the composer instead of above it. Clean `xcodebuild` throughout, confirmed
working by the user.

**Milestone SW9 (attach files/images to prompts) is implemented and committed** (commit `a70e0bc`:
`pi-core-ffi` gained `attach.rs`, porting `pi-core`'s already-proven `attach_path`/
`image_mime_type`/`encode_base64` — the Slint side of this feature was already fully implemented
and tested, so this was purely additive FFI wiring, not new design; `ChatCmd::AttachPath`/
`RemoveAttachment` fire-and-forget like `Send`/`Abort`, `Send`'s handler branches to
`prompt_with_images` when attachments are queued; two new `ChatSink` methods —
`on_pending_attachments_changed` for the chip row and `on_composer_append` for non-image `@path`
references, the latter being the first concrete use of the "push text into the composer" pattern
previously only a hypothetical during SW5's parked `set_editor_text` research) — real `pi`
verification via `spike_check --attach` confirmed the full round trip: an image queues as a chip,
a non-image path correctly routes to `on_composer_append` instead, and the chip clears once
consumed by `send`. Swift side: `ChatView` gained a paperclip button (native `NSOpenPanel`), an
attachment-chip row, and `.dropDestination` for native Finder drag-and-drop. Clean `xcodebuild`,
confirmed working by the user.

**Milestone SW10 (session branch tree + fork-from) is implemented and committed** (commit
`bbcff65`: `pi-core-ffi` gained `tree.rs`, porting `pi-core`'s `fetch_tree_rows`/`flatten_tree`/
`tree_node_summary` against the live `client.get_tree()` RPC — deliberately not built on
`pi_sessions::tree::SessionTree`, whose own doc comment warns its `leaf_id` heuristic can
mis-highlight the active branch after a fork/tree-jump on a live session, and which computes none
of `depth`/`summary`/`label`/`can_fork` anyway; `ChatCmd::OpenTree`/`ForkFrom`; a new
`ChatSink::on_composer_replace`, a genuine replace distinct from SW9's append-only
`on_composer_append`, matching `fork_from`'s `set_composer_text` semantics) — real `pi`
verification via `spike_check --tree` confirmed the full round trip against a real multi-turn
session: correct flattened structure with depth/active/forkable flags, forking from an earlier
user message rewinds the branch, `on_composer_replace` fires with the exact original prompt text,
and history re-hydrates to the earlier turn. Swift side: new `TreeView.swift` sheet (mirrors
`ExtensionDialogView`'s sheet skeleton) with a fork-confirmation alert, reachable via a new
toolbar button (⌘T) in `ChatView`. Clean `xcodebuild`, confirmed working by the user.

This closes out every SW-numbered milestone and named fix in this plan except the markdown
session-export fix, which is parked at the user's request (deprioritized, not dropped) — see its
own section below for the still-accurate plan.

---

## Milestone SW0 — `pi-core` extraction

**Goal:** split `slinty-pi::backend` and its Slint-free sibling modules into a new
`crates/pi-core` crate behind a `UiSink` trait; refactor `slinty-pi` to consume it. Zero behavior
change — `cargo test --workspace` and every `SLINTY_*_AFTER` env hook stay green throughout.

### Verified facts

- `backend.rs`'s only Slint-coupled surface: `Ui` struct (`backend.rs:267-591`, 25 methods, wraps
  `slint::Weak<AppWindow>` + a shadow row counter + a `pub dark: Arc<AtomicBool>` field),
  `RowSpec::to_row()` (`backend.rs:164-190`, builds `slint::StyledText`/`Row`), and
  `code_lines_model`/`table_rows_model` (`backend.rs:196-260`). Confirmed via read: the only
  `use slint::...` in the file is line 15.
- Everything else in `backend.rs` — `UiCmd` (`backend.rs:39-114`, plain enum: `String`/`usize`/
  `PathBuf`/`Secret`), `RowSpec`'s fields (`backend.rs:130-153`, plain data — `code_lines:
  Vec<highlight::CodeLine>` and `table_rows: Vec<Vec<segmenter::TableCell>>` are already
  Slint-free), `Transcript` (event-stream → `RowSpec` projection), `hydrate_rowspecs` (session
  replay), `Sidebar`, local-model orchestration (rapid-mlx/router/HF/Ollama), tree flattening,
  stats, extension-UI dialog handling, and the `pi_backend`/`demo_backend` entry points — only ever
  calls into `Ui`'s setter methods, never touches `slint::`/`AppWindow` directly.
- `segmenter.rs`, `highlight.rs`, `palette.rs`, `density.rs`, `attach.rs`, `demo_sessions.rs`, and
  all of `local/*` (`auth_json.rs`, `hf.rs`, `models_json.rs`, `ollama.rs`, `rapid_mlx.rs`,
  `router.rs`, `system_fit.rs`) have zero `slint::` references — confirmed by grep.
- `pi-rpc` and `pi-sessions` already have zero Slint dependency (confirmed via their `Cargo.toml`
  dependency lists: `tokio`/`serde`/`serde_json`/`thiserror`/`tracing` and
  `notify-debouncer-mini`/`serde`/`serde_json` respectively).
- `Transcript` owns a concrete `ui: Ui` field; roughly 40 free functions in `backend.rs` take
  `&mut Transcript` (never `Ui` generically) — so the extraction seam should be a `Box<dyn UiSink>`
  field, not a generic type parameter threaded through 40 signatures.
- `trash::delete` (used in `delete_session`) and `directories::ProjectDirs` (`density.rs:8`) are
  plain filesystem/config-path logic, not UI-toolkit calls — they move into `pi-core` with the code
  that uses them. `rfd` (folder picker) and `arboard` (clipboard) are genuinely UI-toolkit calls,
  but both call sites already live in `main.rs`, not `backend.rs`, so they stay in `slinty-pi`
  without any code motion.
- `slint::Weak<T>` is unconditionally `Send + Sync` in Slint's own source, so a `Send + Sync` bound
  on the new trait is trivially satisfiable by the Slint-side implementation.
- Existing test coverage in the modules that move is substantial and travels with the code
  unchanged (`git mv` keeps `#[cfg(test)]` blocks in place): `segmenter.rs` (13 tests),
  `highlight.rs` (6), `palette.rs` (5), `density.rs` (3), `attach.rs` (3), `demo_sessions.rs` (2),
  and every `local/*` module (`auth_json.rs` 10, `router.rs` 8, `models_json.rs` 6, `hf.rs` 6,
  `rapid_mlx.rs` 7, `system_fit.rs` 7, `ollama.rs` 3) — all pure parsing/formatting/config-I/O
  logic, no HTTP mocking involved (none of these test the network-calling functions themselves,
  matching this project's existing convention of only integration-testing against real
  processes/services, e.g. `pi-rpc`'s tests skipping when `pi` isn't installed). `backend.rs`
  itself has good coverage of its pure-function pieces moving to `pi-core` (`format_cost`,
  `relative_time`, `is_local_base_url`/`model_label`, server-dot state, tree flattening at
  `tree_tests` (`backend.rs:3385`), and `hydrate_rowspecs` (`hydrate_tests`, `backend.rs:4051`,
  13 tests) — the last of these is a pure `&[Value] -> Vec<RowSpec>` function, so its tests need no
  `Ui` at all.
- **Coverage gap found**: `Transcript`'s *live* event-processing path — the code that turns
  streaming `pi_rpc::Event`s into `RowSpec`s pushed through `self.ui.*` as a session streams
  (text/thinking deltas, tool-call/result matching, compaction, extension-UI dialogs) — has **no
  unit tests today**. The only two places `Ui::new(...)` is constructed are the live
  `pi_backend`/`demo_backend` entry points (`backend.rs:1488`, `backend.rs:3817`), both of which
  need a real `slint::Weak<AppWindow>`, so this logic has only ever been exercised manually (demo
  mode, `SLINTY_*_AFTER` hooks) or through `hydrate_rowspecs`' separate replay path. Turning `Ui`
  into a `UiSink` trait is what makes this testable for the first time.

### Design

- **New crate `crates/pi-core`**, added to `[workspace] members` in the root `Cargo.toml`. Depends
  on `pi-rpc`, `pi-sessions`, and whichever of `pulldown-cmark`/`syntect`/`nucleo-matcher`/
  `reqwest`/`sysinfo`/`base64`/`trash`/`directories`/`chrono` its moved modules need (moved out of
  `slinty-pi/Cargo.toml`). `slinty-pi` keeps `arboard`, `rfd`, `slint`, `i-slint-backend-winit`,
  `winit`, `slint-build` — used only by `main.rs`, which doesn't move.
- **`UiSink` trait** in `pi-core`: a dyn-compatible (`Send + Sync`), mechanical rename of today's
  `impl Ui`'s 25 methods, signatures unchanged. Sync, fire-and-forget methods — mirrors today's
  non-blocking `Weak::upgrade_in_event_loop` semantics exactly; the real UI mutation happens later,
  on whatever thread the implementor hops to (Slint: the Slint event loop; Swift, in SW1+: the
  main actor). `Transcript`'s `ui` field becomes `Box<dyn UiSink>` — this is the only field-type
  change; none of the ~40 functions taking `&mut Transcript` need their signature touched.
- **`dark: Arc<AtomicBool>`** is currently a `pub` field directly on `Ui` (accessed as
  `transcript.ui.dark.load(...)` at call sites) — a trait object can't expose that. Move it to a
  separate `Arc<AtomicBool>` field on `Transcript` itself, cloned once at construction alongside
  `ui: Box<dyn UiSink>`, rather than adding a getter method to `UiSink` for a concern the
  abstraction doesn't otherwise need.
- **What moves verbatim** (`git mv`, then fix `use crate::` paths and any now-cross-crate `pub`
  visibility): `segmenter.rs`, `highlight.rs`, `palette.rs`, `density.rs`, `attach.rs`,
  `demo_sessions.rs`, `local/` in full, and from `backend.rs`: `UiCmd`, `Secret`, `RowSpec` (struct
  only — its Slint-touching `to_row()` stays behind), `Transcript`, `hydrate_rowspecs`, `Sidebar`,
  local-model orchestration, tree/stats/extension-UI handling, `pi_backend`/`demo_backend` (their
  signatures change from `Weak<AppWindow>` to `Box<dyn UiSink>`).
- **What stays in `slinty-pi`**: `Ui` (renamed `SlintUi`, implements `pi_core::UiSink`),
  `RowSpec::to_row()`, `code_lines_model`/`table_rows_model`, and `main.rs` in full (100% Slint
  glue — window setup, the ~30 `app.on_*` callback wirings, the `DropFileHandler` winit handler for
  drag-and-drop, dark-mode sync loop). `main.rs` changes only at its two `rt.spawn(...)` sites:
  construct `SlintUi`, box it, call `pi_core::pi_backend`/`pi_core::demo_backend`.
- **Test coverage requirement**: every module moved into `pi-core` keeps its existing tests
  (verbatim `git mv`), and `pi-core` closes the one real gap found above. Add a `RecordingUiSink`
  test double (`pi-core/src/ui_sink.rs`, behind `#[cfg(test)]` or a small internal `test_util`
  module) implementing `UiSink` by appending every call to a `Vec<UiEvent>` log instead of touching
  any UI — trivial now that `UiSink` is a plain trait instead of a concrete Slint-wrapping struct.
  Use it to add the first-ever unit tests of `Transcript`'s live streaming path: feed synthetic
  `pi_rpc::Event` sequences in (text delta / thinking delta / tool-call start-update-end /
  compaction / extension-UI request) and assert on the resulting `RecordingUiSink` log — mirroring
  the assertion style `hydrate_tests` already uses on `RowSpec` output, just for the streaming path
  instead of the replay path. Any other function moving into `pi-core` that turns out to have no
  test today gets at least one; async functions that only wrap an HTTP/process call (router/
  rapid-mlx/Ollama/HF network calls, `pi_backend`/`demo_backend` themselves) are exempted from unit
  testing per the project's existing convention (integration-shaped, covered by manual/demo-mode
  verification instead) — but every pure function they call out to (parsing, formatting, label
  building, state transitions) is in scope and should already be covered by the tests moving with
  them.

### Work breakdown

1. Scaffold `crates/pi-core` (empty, workspace member, `Cargo.toml` following `pi-sessions`'
   shape: `edition.workspace = true`, `license.workspace = true`).
2. `git mv` the verbatim modules listed above into `pi-core/src/`; fix imports/visibility.
3. Define `UiSink` in `pi-core`; move `UiCmd`, `Secret`, `RowSpec`, `Transcript`,
   `hydrate_rowspecs`, `Sidebar`, local-model orchestration, tree/stats/extension-UI handling,
   `pi_backend`/`demo_backend` into `pi-core`, swapping `Ui` field/param types for `Box<dyn
   UiSink>` and pulling `dark` out to its own `Transcript` field.
4. Implement `SlintUi` in `slinty-pi`, keeping `RowSpec::to_row`/`code_lines_model`/
   `table_rows_model` there.
5. Update both crates' `Cargo.toml`s (dependency moves per Design); add
   `pi-core = { path = "../pi-core" }` to `slinty-pi`.
6. Update `main.rs`'s two spawn sites; fix any remaining compile errors (expected: import-path
   churn only, no logic changes).
7. Add `RecordingUiSink` in `pi-core`; write unit tests for `Transcript`'s live event-processing
   path (streaming text/thinking deltas, tool-call/result matching, compaction markers,
   extension-UI dialog requests) — the coverage gap identified above.
8. Audit every function/module moved into `pi-core` for test coverage; add tests for anything
   found untested that isn't an async network/process wrapper (per the exemption above).
9. Full pass: `cargo fmt`, `cargo clippy --workspace`, `cargo test --workspace`; run
   `SLINTY_DEMO=1 cargo run -p slinty-pi` and spot-check `SLINTY_SEND_AFTER`,
   `SLINTY_SWITCH_PROJECT_AFTER`, `SLINTY_OPEN_MODELS_AFTER`.

### Risks

- **Hidden Slint coupling** missed in the initial audit — mitigated by the compiler: `pi-core`
  simply won't build if it still needs `slint`, so this fails loudly, not silently.
- **Dependency-move edge cases** (a crate needed by both moved code and `main.rs`, e.g. `chrono`)
  — resolve case by case; duplicating a lightweight dep in both `Cargo.toml`s is an acceptable
  fallback over blocking the split.
- **Behavior drift disguised as refactor** — no logic changes permitted in this milestone; any bug
  noticed during the move gets filed, not fixed, to keep the diff reviewable as pure motion.

### Acceptance criteria

- `cargo fmt --check`, `cargo clippy --workspace`, `cargo test --workspace` all green.
- `slinty-pi` behaves identically to pre-refactor: demo mode renders the same synthetic stream;
  every `SLINTY_*_AFTER` env hook still drives the same UI action.
- `pi-core`'s `Cargo.toml` has zero `slint`/`i-slint-*`/`winit` dependency.
- `slinty-pi`'s remaining Slint-coupled surface (`SlintUi` + `to_row`/model-builder helpers) is
  comparable in size to today's measured ~150-160 lines.
- Every pure function/module in `pi-core` has unit test coverage — either carried over from
  `slinty-pi` unchanged or newly added where a gap existed. In particular, `Transcript`'s streaming
  event-processing path has real unit tests for the first time, via `RecordingUiSink`. The only
  untested code in `pi-core` is the thin async glue that wraps a real network/process call
  (consistent with `pi-rpc`'s own integration-test convention), and even those functions have their
  internal pure logic (parsing, formatting, state transitions) covered separately.

---

## Milestone SW1 — FFI spike & minimal SwiftUI chat window

**Goal:** prove one real prompt round-trip through `pi --mode rpc`, end-to-end, from a native
SwiftUI window through a UniFFI boundary — plain-text streaming only, mirroring
`PRODUCT_PLAN.md`'s own M0 spike rather than reaching feature parity. This is the milestone that
retires the biggest unknown (Rust-calls-into-Swift from background threads) before more of
`UiSink` is ever ported across FFI.

### Verified facts

- `pi_rpc::PiClient` already spawns `pi --mode rpc` and exposes a typed `Command`/`Event` stream
  with tolerant serde — reusable unmodified from any Rust caller, including an FFI crate.
- After SW0, `pi_core::demo_backend` is Slint-free and directly reusable as a synthetic-stream perf
  harness for the SwiftUI spike too, without needing `pi` installed — the same role
  `SLINTY_DEMO=1` plays today.
- UniFFI supports **foreign traits** (a Rust trait implemented in Swift, called from Rust on any
  thread) — this is exactly the backend→UI push direction this project needs, and matches the
  `Send + Sync`, fire-and-forget pattern already established for `UiSink` in SW0. The Swift
  implementation is responsible for hopping to `@MainActor` on each callback, the same
  responsibility `Weak::upgrade_in_event_loop` discharges on the Slint side today.

### Design

- **New crate `crates/pi-core-ffi`**, depending on `pi-rpc` directly (not the full `pi-core`
  `Transcript`/`RowSpec`/`UiSink` surface — reconciling with that is explicitly deferred; guessing
  the eventual Swift-facing shape of `RowSpec`/markdown/tables before any real Swift UI exists to
  consume it would be over-designing blind) and `uniffi`.
- **`PiSession` FFI object**: wraps `PiClient::spawn` + its event stream in a small tokio task;
  exports `send(prompt: String)` / `abort()` via `#[uniffi::export]`.
- **`ChatSink` foreign trait** — deliberately smaller than the full `UiSink`:
  ```rust
  pub trait ChatSink: Send + Sync {
      fn on_text_delta(&self, delta: String);
      fn on_turn_end(&self);
      fn on_streaming_changed(&self, streaming: bool);
      fn on_error(&self, message: String);
  }
  ```
- **Swift delivery**: `apps/pi-mac/scripts/build-rust.sh` runs `cargo build -p pi-core-ffi`, then
  `uniffi-bindgen generate --language swift`, then `xcodebuild -create-xcframework`, wired as an
  Xcode "Run Script" build phase (plain build-script route, not a SwiftPM build-tool plugin — fewer
  moving parts while the API shape is still changing; revisit once it stabilizes). Generated
  bindings are not checked in.
- **New SwiftUI app** at `apps/pi-mac/` (top-level, alongside `crates/` — not a Cargo workspace
  member): `PiMac.xcodeproj`, macOS 14+ deployment target, `@Observable` `ChatViewModel`
  implementing the generated `ChatSink` protocol, driving one `ChatView` — composer, Send/Abort,
  a plain-text-appending transcript (`ScrollView`/`Text`, string concatenation, no markdown
  rendering), a streaming status label. No sessions, no local-model panel, no extension-UI dialogs
  — all explicitly out of scope for this milestone.

### Work breakdown

1. `crates/pi-core-ffi`: `PiSession` object wrapping `pi_rpc::PiClient`, `ChatSink` foreign trait,
   `#[uniffi::export]` annotations.
2. `apps/pi-mac/scripts/build-rust.sh`: cargo build → `uniffi-bindgen generate` → xcframework
   packaging.
3. `apps/pi-mac/PiMac.xcodeproj` skeleton, macOS 14+ target, Run Script build phase wired to (2).
4. `ChatViewModel` (`@Observable`, implements the generated `ChatSink` protocol, hops to
   `@MainActor` on every callback).
5. `ChatView`: composer, Send/Abort, transcript, status label.
6. Wire `pi_core::demo_backend` as an optional synthetic-stream mode for the Swift app, for a
   no-`pi`-installed perf check mirroring `SLINTY_DEMO=1`.
7. Verification: real prompt round-trip against an installed `pi`; abort mid-stream; synthetic
   high-rate stream with dropped-frame measurement (Instruments or a frame-time log).

### Risks

- **UniFFI threading edge cases** calling from tokio worker threads into Swift are less
  battle-tested in this codebase than the Slint pattern it already trusts — this milestone is the
  intended risk-retirement exercise; treat friction here as a go/no-go signal before porting more
  of `UiSink` across FFI later.
- **Xcode Run Script reliability** (stale bindings, cargo build failing inside Xcode's sandboxed
  build env) — mitigated by the script failing loudly (non-zero exit fails the Xcode build) and
  being runnable standalone from a terminal for debugging.
- **Scope creep toward full `UiSink` parity** — `ChatSink`'s 4-method surface is a hard boundary
  for this milestone.

### Acceptance criteria

- A real `pi --mode rpc` prompt, sent from the SwiftUI composer, streams into the transcript view
  with no dropped frames at a synthetic 100 token/s rate (mirrors `PRODUCT_PLAN.md`'s M0 bar).
- Abort, sent from the SwiftUI button, actually stops the stream end-to-end.
- `pi-core-ffi`/`pi-rpc` have zero Swift/Objective-C/AppKit code; the Swift app has zero hand-rolled
  protocol/JSONL parsing.
- `apps/pi-mac` builds from a clean checkout via the documented script + Xcode, no manual steps
  beyond having `pi` on `PATH`.

---

---

## Milestone SW2 — Session sidebar in Swift

**Goal:** give the Swift app real session hygiene — browse projects/sessions, switch project,
start fresh, delete, rename the active session — without solving history rendering. **Explicitly
deferred, by decision, not oversight**: resuming an *existing* session's history (needs a rich-
content-over-FFI answer this milestone doesn't have) and the branch-tree/fork-from-message view
(separate machinery, its own follow-on). Because resume is out, "switch to an existing session"
(`SwitchSession`) is *also* out — flipping the active pointer with no visible history is worse than
not offering it.

### Verified facts

- `pi_sessions::MetaCache::list_sessions(&self, session_dir: &Path) -> io::Result<Vec<SessionMeta>>`
  (`crates/pi-sessions/src/scan.rs:278`) is mtime/size-cache-backed and `Sync` via an internal
  `Mutex` — confirmed `pi-core`'s `Sidebar` always calls the cached method, never the free
  `list_sessions` fn. `SessionMeta` (34-46) and `Project` (24-28,
  `dir_name`/`display_path`/`session_dir`) are plain owned-`String`/`PathBuf` structs — trivial to
  mirror as UniFFI `Record`s (`PathBuf` fields convert via `.display().to_string()`, UniFFI has no
  native `PathBuf`). `Project::display_path` is documented lossy (scan.rs:84-88) — display-only,
  never an identity to switch/spawn against; this milestone never needs to, since `switch_project`
  is driven by a real path from `NSOpenPanel`, not by clicking a project row.
- `pi-core::backend::Sidebar` (private `struct`, `backend.rs:1202-1314`) is not part of pi-core's
  public API today. `refresh_sessions_with_active` (1263-1313) computes rows then calls
  `ui.set_sidebar_sessions(rows: Vec<(String, String, String, bool, String)>)` — tuple =
  `(path, title, relative_time, active, cost)`. It has real, already-tested logic: if the active
  session's path isn't in `meta_cache.list_sessions()` yet (pi doesn't write a session file until
  its first message), it synthesizes a placeholder row `(active_path, "New session", "just now",
  true, "")` at index 0 (1289-1310). 4 existing tests cover this (`backend.rs`'s `sidebar_tests`
  module): `no_project_selected_yields_an_empty_sidebar`,
  `lists_both_fixture_sessions_with_none_active_when_active_is_unset`,
  `marks_the_matching_row_active_without_synthesizing_one`,
  `synthesizes_a_row_for_an_active_session_not_yet_on_disk`. `relative_time`/`format_cost`
  (`backend.rs:1430`/`1422`) are the two small formatting helpers this logic calls.
- Session-lifecycle RPCs, read from `pi_core::backend::run_session`'s `UiCmd` handlers (the shape
  to mirror, not call — that code is `Transcript`/`UiSink`-coupled): `SwitchProject` returns
  `SessionOutcome::SwitchProject(path)` up to `pi_backend`'s outer loop, which drops the old
  `PiClient` (its `Drop` kills the child — `kill_on_drop(true)` is set in `pi-rpc`'s
  `PiClient::spawn`) and calls `PiClient::spawn` fresh with `PiOptions { cwd: Some(path), .. }`.
  `NewSession` → `client.new_session(None)`, checks `data["cancelled"]` (an extension can veto).
  `DeleteSession(path)` → `trash::delete(path)` via `spawn_blocking`; if the deleted path was
  active, also `client.new_session(None)` (keeps the live child working against a file that still
  exists). `RenameSession(name)` → `client.set_session_name(name)` — **no path argument**, always
  applies to whatever session the running child currently has open (so only the *active* session is
  renameable without first resuming — matches this milestone's exclusion of resume).
- `crates/pi-core-ffi/src/lib.rs` (SW1, current state) is ~230 lines: `PiSession`
  (`#[derive(uniffi::Object)]`, 50-56) owns a `tokio::runtime::Runtime` + `cmd_tx:
  mpsc::UnboundedSender<ChatCmd>`; `new(sink) -> Result<Self, PiSessionError>` (63-77) calls
  `PiClient::spawn(PiOptions::default())` — **`cwd: None`**, i.e. the child inherits the `.app`
  bundle's process cwd, not a meaningful project directory; there is currently no tracking of "the
  current project" anywhere. `run()` (117-149) is a single flat `loop { tokio::select! }` over one
  `client`/`events` pair for the object's whole lifetime; `ChatCmd` only has `Send(String)`/`Abort`.
  `run_session` in `pi-core` already reassigns loop-scoped `let mut` locals (`models`,
  `thinking_levels`, `streaming`) across its own single loop — direct precedent that `run()` doesn't
  need an outer/inner split to let `SwitchProject` replace `client`/`events` mid-flight.
- UniFFI 0.32 async-exports (`#[uniffi::export(async_runtime = "tokio")]`) don't require an ambient
  tokio runtime to already be running — the macro wraps the future via `async_compat`, which drives
  it on its own (confirmed against `uniffi_macros-0.32.0`'s scaffolding source). So a stateless
  browsing object doesn't need its own `tokio::runtime::Runtime` field the way `PiSession` does.

### Design

- **Extraction lands in `pi-sessions`, not `pi-core`.** `pi-core`'s `Cargo.toml` pulls in `syntect`,
  `pulldown-cmark`, `reqwest`, `sysinfo`, `directories` — none of which the row-computation logic
  touches; it only touches `MetaCache`/`SessionMeta`/`search`. Making `pi-core-ffi` depend on
  `pi-core` just for ~50 lines would drag all of that into the Swift app's staticlib for nothing.
  `pi-sessions` is already the zero-Slint, dependency-light home for pure functions over session
  data, and already has its own fixture files
  (`crates/pi-sessions/tests/fixtures/{basic,branching}.jsonl`) to test against directly.
  New `crates/pi-sessions/src/rows.rs` (re-exported from `lib.rs`):
  ```rust
  #[derive(Debug, Clone, PartialEq)]
  pub struct SidebarRow {   // not `SessionRow` — Slint's generated bindings already have a
      pub path: String,     // same-named type (crates/slinty-pi/src/backend.rs:172, from the
      pub title: String,    // .slint UI file) with identical field names; would be a constant
      pub relative_time: String,  // grep/read collision otherwise.
      pub active: bool,
      pub cost: String,
  }

  pub fn sidebar_rows(meta_cache: &MetaCache, session_dir: &Path, query: &str,
                       active: Option<&str>) -> Vec<SidebarRow> { /* today's Sidebar logic */ }
  ```
  `relative_time`/`format_cost` move alongside it; add `chrono` (same config `pi-core` already
  uses) to `pi-sessions/Cargo.toml`. `pi-core::Sidebar::refresh_sessions_with_active` becomes a
  thin wrapper: call `pi_sessions::sidebar_rows(...)`, map each `SidebarRow` to the existing tuple,
  call `ui.set_sidebar_sessions(rows)` — **`UiSink::set_sidebar_sessions`'s tuple signature is
  unchanged** (3 call sites; the 4 `sidebar_tests` destructure tuples directly and would need
  edits for zero benefit this milestone). `SidebarRow` is the new pi-sessions-owned,
  pi-core-ffi-facing shape; the tuple stays private glue.
- **`SessionIndex`: a second, separate, stateless `#[derive(uniffi::Object)]` in `pi-core-ffi`.**
  Doesn't need a live `pi` child (mirrors `Sidebar::refresh_projects`, which also never touches
  `client`) — lets the sidebar render before/independent of `PiSession` spawning. No
  `tokio::runtime::Runtime` field needed (see async-export fact above); holds just a
  `pi_sessions::MetaCache` (already internally `Mutex`-guarded/`Sync` — a plain field is correct,
  no `Arc<Mutex<>>` needed, and this is exactly how concurrent Swift calls share the warm cache).
  ```rust
  #[derive(uniffi::Record)]
  pub struct ProjectRecord { pub display_path: String }
  #[derive(uniffi::Record)]
  pub struct SessionRecord { pub path: String, pub title: String, pub relative_time: String,
                              pub active: bool, pub cost: String }

  #[derive(uniffi::Object, Default)]
  pub struct SessionIndex { meta_cache: pi_sessions::MetaCache }

  #[uniffi::export(async_runtime = "tokio")]
  impl SessionIndex {
      #[uniffi::constructor]
      pub fn new() -> Self { Self::default() }
      pub async fn list_projects(&self) -> Vec<ProjectRecord> { /* pi_sessions::list_projects */ }
      pub async fn list_sessions(&self, cwd: String, query: String,
                                  active_path: Option<String>) -> Vec<SessionRecord> { /* ... */ }
  }
  ```
  No `current_cwd` param on `list_projects` — "not the current project" is a pure Swift-side
  filtering decision against whatever cwd Swift already tracks, not something `SessionIndex` needs
  to know.
- **`PiSession` gains a real initial cwd and 4 new async, `Result`-returning action methods** — not
  fire-and-forget like `send`/`abort`. Fire-and-forget was fine for those because streaming is
  inherently async/pushed; `switch_project`/`new_session`/`delete_session`/`rename_session` are
  one-shot RPCs with a real completion point, and Swift's plan of "trigger the action, then
  re-fetch the session list" would otherwise race the RPC against the refetch. `oneshot` replies
  fix this cleanly:
  ```rust
  enum ChatCmd {
      Send(String),
      Abort,
      SwitchProject { path: String, reply: oneshot::Sender<Result<(), String>> },
      NewSession { reply: oneshot::Sender<Result<(), String>> },
      DeleteSession { path: String, reply: oneshot::Sender<Result<(), String>> },
      RenameSession { name: String, reply: oneshot::Sender<Result<(), String>> },
  }

  #[uniffi::export(async_runtime = "tokio")]
  impl PiSession {
      pub async fn switch_project(&self, path: String) -> Result<(), PiSessionError> { ... }
      pub async fn new_session(&self) -> Result<(), PiSessionError> { ... }
      pub async fn delete_session(&self, path: String) -> Result<(), PiSessionError> { ... }
      pub async fn rename_session(&self, name: String) -> Result<(), PiSessionError> { ... }
  }
  ```
  `PiSessionError` gains an `Action(String)` variant for these. `new(sink)`'s constructor also
  takes an initial `cwd: String` (Swift passes a persisted "last project" or a sensible default —
  deciding that default is part of this milestone's work, not an afterthought) instead of
  `PiOptions::default()`'s bare `cwd: None`.
- **`run()` stays a single flat loop** — `client`/`events` become `let mut` locals reassigned in
  the `SwitchProject` arm (precedented by `run_session`'s own loop-scoped mutable locals). Doesn't
  drop the old `client` until the new one spawns successfully, so a failed `switch_project` leaves
  the session fully usable rather than clientless:
  ```rust
  ChatCmd::SwitchProject { path, reply } => {
      match PiClient::spawn(PiOptions { cwd: Some(PathBuf::from(&path)), ..Default::default() }).await {
          Ok((c, e)) => {
              client = c; events = e; // old client (+ child, kill_on_drop) dropped here
              sink.on_active_session_changed(active_session_path(&client).await);
              let _ = reply.send(Ok(()));
          }
          Err(e) => { let _ = reply.send(Err(e.to_string())); } // old client/events untouched
      }
  }
  ```
  `NewSession`/`DeleteSession`/`RenameSession` are new match arms needing no respawn, just RPC
  calls on the existing `client` (`DeleteSession` needs a `trash` dependency added to
  `pi-core-ffi/Cargo.toml`, and a small local `active_session_path`/delete helper — reimplemented
  here rather than pulled from `pi-core`, matching this crate's existing "doesn't depend on
  pi-core" posture, e.g. its own `chunks()` helper).
- **New `ChatSink` method replaces the originally-sketched bare reset signal:**
  `fn on_active_session_changed(&self, path: Option<String>)`. Fired once right after the initial
  spawn and after every successful `SwitchProject`/`NewSession`/`DeleteSession`-of-the-active-
  session. Swift's contract: clear `transcript`, and feed this value into `SessionIndex.
  list_sessions`'s `active_path` param for row highlighting — strictly more useful than a bare
  "something changed" signal, and it's `SessionIndex`'s only missing input. (The sidebar's own
  project/session *lists* refresh by Swift explicitly re-calling `SessionIndex` after an action
  completes — pull-based, no separate push notification needed for list contents, only for the
  active-session pointer.)
- **Demo mode** (`run_demo`): must handle the 4 new `ChatCmd` variants explicitly (reply `Ok(())`,
  fire `on_active_session_changed(None)` where relevant) rather than silently dropping their
  `reply` senders in the existing `let ChatCmd::Send(_) = cmd else { continue }` — a dropped
  `oneshot` resolves Swift's `await` to an error, so demo mode would otherwise look broken for
  every sidebar action.
- **SwiftUI**: `NavigationSplitView` — sidebar shows the current project name + a button opening
  `NSOpenPanel` (native folder picker, replacing the Slint app's `rfd` call site), a `List` of
  `SessionRecord` (non-active rows visually non-interactive, since resume isn't wired up — e.g.
  dimmed, no tap action), a "New Session" toolbar button, delete via swipe/context-menu, rename via
  a context-menu text field on the active row only. Detail pane is the existing `ChatView`.

### Work breakdown

1. `crates/pi-sessions`: new `rows.rs` (`SidebarRow`, `sidebar_rows`, relocated
   `relative_time`/`format_cost`), add `chrono` dep, re-export from `lib.rs`. Tests against the
   crate's own fixtures (mirroring/moving the 4 existing `sidebar_tests` cases).
2. `pi-core`: `Sidebar::refresh_sessions_with_active` becomes a thin wrapper over
   `pi_sessions::sidebar_rows`; delete the now-relocated private `relative_time`/`format_cost`.
   Zero behavior change — full `cargo test -p pi-core` green throughout.
3. `pi-core-ffi/Cargo.toml`: add `pi-sessions`, `trash`.
4. `pi-core-ffi`: `SessionIndex` object (`ProjectRecord`/`SessionRecord`, `list_projects`/
   `list_sessions`); `PiSession::new` takes an initial `cwd: String`; `ChatCmd` gains the 4
   reply-carrying variants; `run()`'s `SwitchProject`/`NewSession`/`DeleteSession`/`RenameSession`
   arms; `ChatSink::on_active_session_changed`; `PiSessionError::Action`; `run_demo` handles the
   new variants explicitly.
5. Regenerate bindings (`scripts/build-rust.sh`), confirm the new Swift API surface
   (`SessionIndex`, the 4 new `async throws` `PiSession` methods, `onActiveSessionChanged`) matches
   expectations.
6. SwiftUI: `NavigationSplitView` restructure, project picker (`NSOpenPanel`), session `List`,
   new/delete/rename actions wired to the new async methods + list refetch, `ChatViewModel` clears
   `transcript` on `onActiveSessionChanged`.
7. Decide and implement the initial-cwd default (persisted last project via `UserDefaults`, or
   fall back to a sensible default like the user's home directory or a "pick a project" empty
   state on first launch).

### Risks

- **Fire-and-forget-vs-request/response mismatch** was the one real bug in the initial design
  sketch (an action racing a pull-based list refresh) — mitigated by making the 4 new methods
  `async`/`Result`-returning from the start, not discovered after the fact.
- **`run()` mutable-local reassignment correctness** — `tokio::select!` re-evaluates `events.recv()`
  fresh every loop iteration, so this is sound, but it's new-ish code; cover with a `pi-core-ffi`
  unit test analogous to the existing `run_demo_tests` (spawn `run` against a fake/demo client
  pair if feasible, or at minimum manually verify a real `switch_project` round-trip the way SW1's
  `spike_check.rs` example verified `send`/`abort`).
- **First-launch cwd default** is a real product decision (not just plumbing) — resolve explicitly
  in step 7 rather than defaulting to something arbitrary and forgetting to revisit it.

### Acceptance criteria

- `cargo fmt --check && cargo clippy --workspace && cargo test --workspace` green; `pi-core`'s 4
  `sidebar_tests` (or their `pi-sessions`-relocated equivalents) still pass, proving zero behavior
  change to the Slint app's sidebar.
- A real `pi --mode rpc` session: switching project respawns the child in the new cwd with a fresh
  transcript; new session clears the transcript; deleting the active session both removes the file
  and starts a fresh one; deleting a non-active session doesn't disturb the current transcript;
  renaming the active session is reflected in the session list on next refresh.
- `apps/pi-mac` builds clean via `xcodebuild -scheme PiMac build` from a clean checkout, same as
  SW1's bar.

---

## Milestone SW3 — History hydration + markdown/code rendering parity, and restoring/switching sessions

**Goal:** replace the Swift app's plain-text-only transcript with the same rich rendering the
Slint app has (markdown prose/headings/quotes/rules, syntax-highlighted code, tables, tool-call/
thinking/info rows), and use it to unlock resuming a session's history — both when the app
launches (**restoring** the last-active session) and when the user clicks a different row in the
sidebar (**switching**). These were deferred out of SW2 specifically because this decision hadn't
been made yet.

**Rendering strategy decision (resolved):** reuse `pi-core`'s Rust segmenter/highlighter/hydration
pipeline across FFI as typed data, rather than reimplementing markdown segmentation and syntax
highlighting natively in Swift. Rationale, from research into `crates/pi-core/src/{segmenter,
highlight,backend}.rs`:
- The data these pieces produce is already flat, plain-data, and FFI-friendly — `Segment`,
  `TableCell`, `CodeLine`, `ColoredSpan` are all `String`/`bool`/`u8`/`Vec` with no lifetimes or
  Slint types — mapping onto UniFFI `Record`s is close to mechanical.
- `highlight_lines` already resolves every span to a concrete `(u8, u8, u8)` RGB triple before
  returning — a Swift consumer needs zero syntect theme/asset data. Reimplementing highlighting
  natively (`Highlightr`/`Splash`) would mean re-sourcing or hand-porting syntect's bundled
  `base16-ocean.dark`/`InspiredGitHub` themes from scratch, with guaranteed visual drift.
  `RowSpec.markdown` (used for `prose`/`quote` rows) deliberately leaves *inline* markdown (bold/
  italic/links/lists) unresolved for the UI toolkit's own renderer to handle — this is the one
  piece Swift needs a native answer for regardless of which option is chosen, and SwiftUI's
  built-in `AttributedString(markdown:)` (available since macOS 12, well within this project's
  macOS 14+ target) covers it.
- `hydrate_rowspecs` (`crates/pi-core/src/backend.rs:938-1110`) is ~170 lines of message-shape
  knowledge — mapping `get_messages`' raw `AgentMessage[]` JSON (role-tagged: `user`/`assistant`/
  `toolResult`/`bashExecution`/`compactionSummary`/`branchSummary`/`custom`) into `RowSpec`s,
  including tool-call/result matching by id (a `toolResult` message patches an *earlier* `toolCall`
  row in place rather than appending a new one) and truncation limits (`TOOL_DETAIL_LIMIT`).
  Reimplementing this in Swift means hand-parsing pi's raw wire-protocol message JSON outside
  `pi-rpc`'s typed layer — directly against this project's own stated policy (CLAUDE.md,
  "Extending the RPC protocol surface": add typed serde variants in Rust, don't parse raw `Value`
  in frontend code) and a second copy of logic that will drift from `pi-core`'s as pi's message
  format evolves.

### Verified facts

- **`hydrate_rowspecs`** (`crates/pi-core/src/backend.rs:938-1110`, free fn, called only from
  `Transcript::hydrate` at line 336-340) — signature
  `fn hydrate_rowspecs(messages: &[serde_json::Value], dark: bool) -> Vec<RowSpec>`. Its doc
  comment: "turn a `get_messages` payload (`AgentMessage[]`, per `docs/session-format.md`) into
  RowSpecs. Same building blocks as the live streaming path (`spec_for_segment`, `tool_summary`,
  `content_text`)." Complete set of `RowSpec.kind` values across both the hydration and live paths:
  `"user"`, `"info"`, `"error"`, `"thinking"`, `"tool"`, `"prose"`, `"heading"`, `"code"`,
  `"quote"`, `"rule"`, `"table"` — `"note"` is not itself a kind, `RowSpec::note(kind, text)`
  (line 156-164) is a generic constructor used to build `"user"`/`"info"`/`"error"` rows.
- **`resume_session`** (`backend.rs:1830`) — `client.switch_session(session_path).await` →
  check `data["cancelled"] == true` (an extension can veto) → info-note and bail if cancelled,
  error-note and bail on RPC failure → otherwise `hydrate_active_session`. **`hydrate_active_session`**
  (`backend.rs:1873`) — `client.get_messages().await` → `transcript.reset()` (clears live-stream
  state, calls `ui.clear()`) → `transcript.hydrate(&messages)` → `update_stats`. The same
  `hydrate_active_session` helper is reused by `clone_session` and `fork_from`, both after their
  own cancelled-check. `Transcript::hydrate` (line 336) does exactly one bulk call:
  `self.ui.push_all(hydrate_rowspecs(messages, dark))` — all chunking is the `UiSink`
  implementor's choice (Slint's `SlintUi::push_all`, `slinty-pi/src/backend.rs:71-85`, batches in
  groups of 100 to keep the event loop responsive).
- **`PiClient`** (`crates/pi-rpc/src/client.rs`) has direct, reusable methods for every RPC this
  milestone needs: `get_messages(&self) -> Result<Value, PiError>` (322),
  `switch_session(&self, session_path: impl Into<String>) -> Result<Value, PiError>` (331, doc:
  "only valid within the same cwd — a project change requires a respawn"),
  `new_session(&self, parent_session: Option<String>) -> Result<Value, PiError>` (340). All
  response payloads are raw `serde_json::Value`, not typed structs — only `Event`/
  `AssistantMessageEvent`/`Command` are typed.
- **`Event`** (`pi-rpc/src/types.rs:206-322`) has the full set of streaming variants beyond what
  `pi-core-ffi`'s `apply()` currently handles: `AgentStart`, `AgentEnd`, `AgentSettled`,
  `TurnStart`, `TurnEnd`, `MessageStart`, `MessageUpdate`, `MessageEnd`, `ToolExecutionStart/
  Update/End`, `QueueUpdate`, `CompactionStart/End`, `AutoRetryStart/End`, `ExtensionUiRequest`,
  `ExtensionError`, `Unknown` (`#[serde(other)]` fallback, line 320). `pi-core-ffi`'s current
  `apply()` (`crates/pi-core-ffi/src/lib.rs:410-425`) only forwards `AgentStart`/`AgentSettled`/
  `TextDelta`/`MessageEnd`/non-aborted `Error` — it never sees tool-call or thinking events today.
- **`UiSink`** (`backend.rs:175-227`, 24 methods) is far larger than what this milestone needs —
  most methods concern local-model panels, the palette, and sidebar/tree state already handled by
  SW2's `SessionIndex`/`PiSession`. Only the row-list methods are relevant: `push`, `set`,
  `push_all`, `clear`, `truncate`.
- `RowSpec` (`backend.rs:131-154`) fields confirmed current: `kind: &'static str, markdown:
  Option<String>, text: String, lang: String, level: i32, detail: String, running: bool, elapsed:
  String, first: bool, raw: String, code_lines: Vec<highlight::CodeLine>, table_rows:
  Vec<Vec<segmenter::TableCell>>`. `spec_for_segment` (`backend.rs:839-887`) maps a
  `segmenter::Segment` (`Prose(String) | Heading{level,text} | Code{lang,code} | Quote(String) |
  Rule | Table(Vec<Vec<TableCell>>)`, from `segment_markdown(&str) -> Vec<Segment>`,
  `segmenter.rs:45`) to a `RowSpec`, calling `highlight::highlight_lines(code, lang, dark)`
  (`highlight.rs:66`) for `Code` segments. `CodeLine { spans: Vec<ColoredSpan> }`, `ColoredSpan {
  text: String, color: (u8,u8,u8) }` (`highlight.rs:18-27`) — colors are pre-resolved, no theme
  data needed downstream. `TableCell { text: String, header: bool }` (`segmenter.rs:16-20`).
  `row_to_slint` (`slinty-pi/src/backend.rs:339-366`) is the existing Slint-side consumer to mirror
  conceptually — note its `table_rows_model` (401-436) also computes per-column width weights
  in Rust today; that logic is Slint-specific (pixel-width estimation for Slint's layout engine)
  and won't be reused as-is, but the *data* (`TableCell` rows) crossing FFI is what matters here.
- **`pi-core-ffi`'s current posture** (`crates/pi-core-ffi/src/lib.rs`, crate doc comment) is
  explicitly "doesn't depend on `pi-core`" — deliberately deferred in SW1 ("guessing the eventual
  Swift-facing shape of markdown/table rendering before any real Swift UI exists to consume it
  would be over-designing blind"). SW3 is the milestone that resolves that deferral, but per SW2's
  own precedent (extracting `sidebar_rows` into lean `pi-sessions` rather than pulling all of
  `pi-core` — with its `reqwest`/`sysinfo`/`directories`/`nucleo-matcher` — into the Swift app's
  staticlib for ~50 lines of logic), the ~700 lines this milestone needs (`segmenter.rs`,
  `highlight.rs`, `RowSpec`, `spec_for_segment`, `hydrate_rowspecs`, and their shared helpers
  `content_text`/`user_content_text`/`tool_summary`) should move into a new lean crate rather than
  pulling in all of `pi-core`.
- `PiSession::new` (`lib.rs:98-115`) currently takes `(sink, cwd)` with no way to resume a specific
  session on construction — mirrors `pi_backend`'s `resume_on_first_spawn: Option<String>` param
  (`backend.rs:1447-1449`), which SW3 needs an FFI-facing equivalent of for launch-time restore.
  `ChatCmd`/`run()`'s existing `SwitchProject`/`NewSession`/`DeleteSession`/`RenameSession` arms
  (SW2) are the direct pattern to extend for a new `SwitchSession` arm.

### Design

- **New crate `crates/pi-render`** (workspace member, depends only on `pulldown-cmark` and
  `syntect` — no `reqwest`/`sysinfo`/`directories`/`nucleo-matcher`). `git mv` `segmenter.rs`,
  `highlight.rs` from `pi-core` into it; move `RowSpec`, `spec_for_segment`, `hydrate_rowspecs`,
  and their shared helpers (`content_text`, `user_content_text`, `tool_summary`, the tool-output
  truncation constant) out of `backend.rs`. `pi-core` adds a `pi-render` dependency and keeps using
  these types unchanged (`Transcript`/`UiSink` still operate on `pi_render::RowSpec` — the live
  streaming path in `pi-core` calls the same `spec_for_segment`/helpers it always did, just from
  their new home). Named `pi-render` (not `pi-transcript`) to avoid colliding with `pi-core`'s
  existing stateful `Transcript` struct, which stays put — `pi-render` only holds the stateless
  message/segment → `RowSpec` construction logic. Zero behavior change to the Slint app; its
  existing test coverage (`segmenter.rs` 13 tests, `highlight.rs` 6 tests, `hydrate_rowspecs`'
  `hydrate_tests` 13 tests) moves verbatim and must stay green — same discipline as SW0's own
  extraction.
- **`pi-core-ffi` adds a `pi-render` dependency** and defines UniFFI `Record` mirrors with `From`
  conversions (same pattern as SW2's `SessionRecord: From<pi_sessions::SidebarRow>` in
  `session_index.rs`):
  ```rust
  #[derive(uniffi::Record)]
  pub struct ColoredSpanRecord { pub text: String, pub red: u8, pub green: u8, pub blue: u8 }
  #[derive(uniffi::Record)]
  pub struct CodeLineRecord { pub spans: Vec<ColoredSpanRecord> }
  #[derive(uniffi::Record)]
  pub struct TableCellRecord { pub text: String, pub header: bool }
  #[derive(uniffi::Record)]
  pub struct RowRecord {
      pub kind: String, pub markdown: Option<String>, pub text: String, pub lang: String,
      pub level: i32, pub detail: String, pub running: bool, pub elapsed: String,
      pub first: bool, pub raw: String,
      pub code_lines: Vec<CodeLineRecord>, pub table_rows: Vec<Vec<TableCellRecord>>,
  }
  ```
  UniFFI 0.32's support for a doubly-nested `Vec<Vec<Record>>` (`table_rows`) hasn't been
  confirmed against this exact version — verify during implementation; fallback is flattening to
  a row-major `Vec<TableCellRecord>` plus a `columns: u32` if nesting proves awkward.
- **`ChatSink` gains one method**: `fn on_history_replaced(&self, rows: Vec<RowRecord>)`. Not the
  full `push`/`set`/`push_all`/`clear`/`truncate` `UiSink` surface — this milestone's design (next
  bullet) only ever needs "replace the whole rendered transcript," never in-place row edits, so one
  method covers it. `on_active_session_changed`'s SW2-era contract ("Swift's contract: clear the
  visible transcript") is narrowed: it becomes purely a path/sidebar-highlighting signal, no longer
  responsible for clearing rows — `on_history_replaced` is the single source of truth for "what's
  currently in the transcript," always called exactly once by every session-changing action.
- **One shared hydrate-and-push helper, called from four places**, all reusing
  `resume_session`/`hydrate_active_session`'s exact shape (`get_messages` → `hydrate_rowspecs` →
  push): initial construction (if a resume path is given — see next bullet), `switch_session`,
  and, replacing `switch_project`/`new_session`/`delete_session`(-of-active)'s current "fire
  `on_active_session_changed`, let Swift clear its own transcript" contract, those four SW2 actions
  too — every one of them now ends with a real `on_history_replaced` call (empty for a fresh
  session, populated for a resumed one) instead of relying on Swift-side clearing.
  ```rust
  async fn hydrate_and_push(client: &PiClient, sink: &dyn ChatSink, dark: &AtomicBool) {
      match client.get_messages().await {
          Ok(data) => {
              let messages = data.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
              let rows = pi_render::hydrate_rowspecs(&messages, dark.load(Ordering::Relaxed));
              sink.on_history_replaced(rows.into_iter().map(RowRecord::from).collect());
          }
          Err(e) => sink.on_error(format!("could not load session messages: {e}")),
      }
  }
  ```
- **Restoring (launch-time) vs. switching (sidebar click) is the same underlying action** —
  `PiSession::new` gains a `resume_session_path: Option<String>` parameter (mirroring
  `pi_backend`'s `resume_on_first_spawn`); when set, `run()` calls `client.switch_session(path)` +
  the cancelled-check + `hydrate_and_push` once at startup, before entering its event loop.
  Switching is the same `client.switch_session(path)` + cancelled-check + `hydrate_and_push`,
  exposed as a new async `PiSession::switch_session(path: String) -> Result<(), PiSessionError>`
  (same `oneshot`-reply pattern as SW2's `switch_project`/etc.) driven by a new `ChatCmd::
  SwitchSession { path, reply }` arm in `run()`. `AppModel` persists the last *active session path*
  in `UserDefaults` (new key, alongside SW2's last-project key) and passes it as
  `resume_session_path` on `start()`.
- **Turn-finalization reuses the same `hydrate_and_push` call, on `Event::AgentSettled`.** Rather
  than porting `pi-core::Transcript`'s live incremental flush machinery (33ms/100ms coalesced
  `push`/`set` calls, per-tool-call-id state tracking) across FFI — a large, risk-heavy undertaking
  for one milestone — `run()`'s event branch calls `hydrate_and_push` once whenever `AgentSettled`
  fires (in addition to the existing `on_streaming_changed(false)`), re-fetching and re-rendering
  the *entire* current transcript from `get_messages()` truth. This sidesteps needing to
  incrementally patch tool-result rows into place client-side (already handled correctly by
  `hydrate_rowspecs` operating on the full, final message list) at the cost of re-transferring/
  re-highlighting the whole session on every turn — acceptable for realistic session sizes,
  revisit with an incremental tail-append (tracking a "rows already shown" cursor) later if
  profiling shows otherwise (see Risks).
- **Live in-flight text stays exactly as it is today** (`on_text_delta` accumulating into an
  ephemeral, plain-text "streaming" bubble in `ChatView`) — this milestone does not attempt
  real-time rich rendering of a message still being typed. The ephemeral bubble is cleared once
  `AgentSettled`'s `on_history_replaced` delivers the finalized, richly-rendered row for that same
  content. Explicit, intentional scope reduction: no live "code block formats as it streams"
  parity with Slint's 33ms flush, and no live tool-call "running" spinner mid-turn (only the
  existing top-level streaming status dot signals activity) — both notable, deferred follow-ups if
  found lacking in practice, not oversights.
- **Dark-mode threading**: `PiSession` tracks `dark: Arc<AtomicBool>`, read at every
  `hydrate_and_push` call (mirrors `pi-core::Transcript`'s own `dark` field/pattern exactly). New
  `PiSession::set_dark_mode(&self, dark: bool)` UniFFI method; `AppModel` calls it once at startup
  from the initial `colorScheme` and again via SwiftUI's `.onChange(of: colorScheme)`. A theme
  flip doesn't retroactively re-color already-rendered rows until the next hydrate/settle — an
  accepted minor staleness, not different in kind from how Slint reads the flag fresh per flush.
- **SwiftUI rendering**: a `RowView` (or small family of per-kind views) switching on
  `RowRecord.kind`. `prose`/`quote` → `Text(AttributedString(markdown: row.markdown ?? row.text))`
  (SwiftUI's native Markdown init handles bold/italic/links/lists — the one piece FFI reuse still
  needs a native answer for, per the rendering-strategy rationale above). `heading` → `Text` sized
  by `row.level`. `code` → a monospaced card view iterating `row.codeLines`/`.spans`, `Text(span.
  text).foregroundStyle(Color(red:green:blue:))`. `table` → a simple `Grid`/`LazyVGrid` over
  `row.tableRows`. `thinking`/`tool`/`user`/`info`/`error`/`rule` → simple styled `Text`/`HStack`
  rows, no need to match Slint's exact card chrome. `ChatView` replaces its single
  `Text(model.transcript)` with a scrollable list over `model.rows: [RowRecord]`, plus the
  existing ephemeral streaming-text view below it for the in-flight turn.
- **`SidebarView`** (SW2): non-active session rows become tappable, calling a new
  `model.switchSession(to:)` (→ `PiSession.switchSession(path:)` + a sidebar refetch) — removes
  the "deliberately not click-to-resume" restriction and its comment.

### Work breakdown

1. Extract `crates/pi-render`: `git mv` `segmenter.rs`/`highlight.rs`, move `RowSpec`/
   `spec_for_segment`/`hydrate_rowspecs`/shared helpers out of `pi-core/src/backend.rs`; `pi-core`
   depends on it, uses the moved types unchanged. Zero behavior change —
   `cargo test --workspace` green, moved tests (32 total) pass verbatim.
2. `pi-core-ffi/Cargo.toml`: add `pi-render`. Define `RowRecord`/`CodeLineRecord`/
   `ColoredSpanRecord`/`TableCellRecord` + `From<pi_render::RowSpec>` (and nested) conversions;
   unit-test the conversions directly (no FFI needed for this).
3. `ChatSink`: add `on_history_replaced(rows: Vec<RowRecord>)`; update its doc comment and
   `on_active_session_changed`'s to reflect the narrowed clearing contract.
4. `PiSession`: add `dark: Arc<AtomicBool>` field + `set_dark_mode` method; implement
   `hydrate_and_push` as a shared async helper.
5. `PiSession::new` gains `resume_session_path: Option<String>`; on a `Some`, `run()` resumes +
   hydrates once before entering its event loop (cancelled-check per `resume_session`'s reference
   behavior). Add `ChatCmd::SwitchSession { path, reply }` + `PiSession::switch_session(path)`
   async method, same shape as SW2's other lifecycle actions.
6. `run()`: `switch_project`/`new_session`/`delete_session`(-of-active) arms call
   `hydrate_and_push` instead of relying on Swift-side clearing; `Event::AgentSettled` branch also
   calls `hydrate_and_push` after `on_streaming_changed(false)`.
7. `run_demo`: fire `on_history_replaced` with a couple of synthetic demo rows (e.g. one `prose`
   + one `code` row) so the new Swift rendering path is exercisable without a real `pi`; extend
   `reply_demo_action`/tests for the new `SwitchSession` variant.
8. Regenerate bindings (`scripts/build-rust.sh`); `swiftc -typecheck` against the generated
   bindings to confirm the new Swift-facing API shape (`RowRecord`, `onHistoryReplaced`,
   `switchSession`, `PiSession.new`'s new parameter) before touching Xcode/SwiftUI code.
9. SwiftUI: `RowView` family (per-kind rendering per Design); `ChatView` switches from
   `Text(model.transcript)` to a scrollable `model.rows: [RowRecord]` list + the retained
   ephemeral streaming bubble.
10. `AppModel`: `rows: [RowRecord]` published state, `onHistoryReplaced` implementation, persist/
    read last-active-session-path via `UserDefaults`, pass it as `resume_session_path` on
    `start()`, wire `set_dark_mode` (initial + `.onChange(of: colorScheme)`), add
    `switchSession(to:)`.
11. `SidebarView`: make non-active rows tappable → `model.switchSession(to:)`; remove the
    click-to-resume restriction/comment from SW2.
12. Verification: extend `examples/spike_check.rs` with a `history` mode (send a prompt with a
    code block, `switch_session` back to it, assert `on_history_replaced` fires with the expected
    row kinds); full `cargo fmt/clippy/test --workspace`; real `xcodebuild -scheme PiMac build`;
    manual real-`pi` pass — switch to an older session in the sidebar and confirm it renders richly
    (matching what the Slint app shows for the same session file), send a prompt that produces a
    code block/table and confirm it renders correctly once the turn settles, relaunch the app and
    confirm the last-active session auto-restores.

### Risks

- **Nested `Vec<Vec<TableCellRecord>>` over UniFFI 0.32** — unconfirmed against this exact
  version; flatten to row-major + `columns: u32` if it proves unsupported or awkward.
- **Full re-hydrate-on-every-`AgentSettled` cost** (re-fetching, re-segmenting, re-highlighting,
  and re-transferring the *entire* session on every turn, not just the new tail) — a deliberate
  correctness-first simplification (sidesteps needing to replicate `hydrate_rowspecs`' tool-result-
  patches-earlier-row logic against a partial tail). Acceptable for realistic session sizes;
  mitigate with an incremental "rows already shown" cursor later if real-`pi` testing or profiling
  shows it's laggy on large sessions.
- **`AgentSettled` reliability after abort/error paths** — SW1 already found this specific `pi`
  installation has model/config-dependent event-shape quirks (tool-call-heavy prompts route
  differently than plain conversational ones); verify empirically during step 12 that
  `AgentSettled` fires reliably enough after an abort to trigger the finalization hydrate, not just
  on a clean turn completion.
- **Deferred live rendering fidelity** — no live rich-content or tool-call-spinner rendering
  mid-turn (Design section, explicit scope reduction) could feel like a regression for long,
  tool-call-heavy turns where the only feedback is the top-level streaming dot; revisit if this
  proves to matter in practice.
- **Scope size** — this is the largest milestone yet (12 steps vs. SW0-SW2's 6-9); the "reviewed
  and committed individually" convention still applies per-step, and steps 1-3 (pure Rust,
  zero-Swift-facing) can land and be verified independently of steps 8-12 (Swift-facing) if the
  milestone needs to be split across sessions.

### Acceptance criteria

- `cargo fmt --check && cargo clippy --workspace && cargo test --workspace` green; `pi-render`'s
  moved tests (segmenter 13, highlight 6, `hydrate_tests` 13 — 32 total) pass unchanged, proving
  zero behavior change to the Slint app's rendering.
- Real `pi --mode rpc` verification: clicking an older session in the Swift sidebar renders its
  history with the same row kinds/content the Slint app shows for that session file (prose,
  headings, code with syntax highlighting, tables, tool-call rows); a fresh prompt producing a code
  block renders it correctly once the turn settles; quitting and relaunching the app restores the
  last-active session's rendered history automatically, matching `pi_backend`'s
  `resume_on_first_spawn` behavior.
- `apps/pi-mac` builds clean via `xcodebuild -scheme PiMac build` from a clean checkout, same bar
  as SW1/SW2.

---

## Milestone SW4 — Local-model panel in Swift

**Goal:** bring the Slint app's "local models" management panel — rapid-mlx status/cached
models/serve, a llama.cpp router's models/load/unload, Hugging Face GGUF search/download, Ollama
detection/bulk-add, and cloud-provider API key entry — to the Swift app, reusing `pi-core`'s
`local::*` clients and pure formatters rather than reimplementing HTTP/CLI/file-I/O logic a second
time. **Explicitly out of scope, by decision**: a composer "current active model" picker and the
status-bar "server dot" health indicator — both depend on tracking *which* model pi currently has
selected, a concept the Swift app has no UI for yet and which SW4's own named scope ("local-model
**panel**") doesn't call for. This panel is browse/manage-only; "what is pi using right now"
visibility is a natural follow-up once a picker exists, not an oversight here.

### Verified facts

- **`crates/pi-core/src/local/`** (`mod.rs` + 7 submodules, ~2465 lines) is already toolkit-agnostic
  Rust with zero `Transcript`/`UiSink` coupling — it's nested inside `pi-core` only because nothing
  needed it elsewhere yet. Each submodule pairs pure parsing/formatting (well unit-tested, 47 tests
  total per the project's SW0-era coverage: `auth_json` 10, `router` 8, `models_json` 6, `hf` 6,
  `rapid_mlx` 7, `system_fit` 7, `ollama` 3) with a small number of impure async I/O methods on a
  plain struct — no frontend state, no `PiClient` dependency anywhere in `local::*` itself:
  - `auth_json.rs` (375 lines) — `AuthJson::{load_or_empty, entries, set_api_key, write}` against
    `~/.pi/agent/auth.json`; atomic 0600 write+`.bak`; `entries()` returns `(provider, KeyForm)`
    only, never key material; `set_api_key` refuses `$ENV`/`!command`/OAuth entries
    (`AuthJsonError::Protected`).
  - `hf.rs` (265 lines) — `HfSearch::search_gguf(query, limit)` (`GET
    https://huggingface.co/api/models?search=...&filter=gguf&full=true&limit=...`, bearer-auth via
    `HF_TOKEN` env if set); pure `gguf_quants(&HfModel) -> Vec<String>` parses quant labels
    (`Q4_K_M`, etc.) from `.gguf` sibling filenames.
  - `models_json.rs` (215 lines) — same load/parse/`set_provider`/write contract as `auth_json`, for
    `~/.pi/agent/models.json`'s `providers` map (not 0600 — not a secrets file).
  - `ollama.rs` (166 lines) — `OllamaProbe::list_models()` (`GET http://localhost:11434/api/tags`,
    5s timeout, any failure → `None`); pure `provider_preset(&[String]) -> Value` builds pi's
    `models.json` `ollama` provider block.
  - `rapid_mlx.rs` (711 lines, largest) — `RapidMlx::{version, running_servers, cached_models,
    catalog, info}` shell out to the `rapid-mlx` CLI (text-scraped, no `--json` flag exists) via a
    private `run()`; `server_health(base_url)` HTTP-probes a running server's `/health` (`ready`/
    `model_loaded`/`model_name` — needed because rapid-mlx 404s completions for any model other
    than the one it's currently serving); `ManagedServer::{spawn, wait_ready, is_alive, shutdown}`
    owns one `rapid-mlx serve <alias> --port <port>` child (one model per process, `kill_on_drop`
    expected — confirm exact `Command` construction when porting).
  - `router.rs` (604 lines) — `LlamaRouter::{health, list_models, load_model, unload_model,
    download_model}` against a llama.cpp-server router (`http://127.0.0.1:8080` default);
    `download_model` is literally `POST /models` (starts a download, doesn't load); an SSE
    subscription path exists but is dead code (`#[allow(dead_code)]`) — the reference implementation
    polls `GET /models` instead.
  - `system_fit.rs` (129 lines) — `SystemMemory::probe()` (via `sysinfo`) + pure `fit_label`/
    `human_size` feeding the cached-model "fits / may be slow / won't fit" pills.
- **The pure panel-formatting layer currently lives in `pi-core/src/backend.rs`, not `local/`**, and
  is exactly what both frontends need identically: `RapidMlxPanelData`/`RouterPanelData` structs
  (lines 198-215: `cached` rows are `(alias, hf_repo, human_size, fit_label)`; `models` rows are
  `(id, status_label, loaded, busy)`), `RapidMlxSnapshot` (private), `collect_rapid_mlx_snapshot`
  (1678-1690, impure — calls `RapidMlx::{version,running_servers,cached_models,catalog}`),
  `format_rapid_mlx_panel` (1692-1719, pure), `fetch_router_state` (1794-1810, impure — `router
  .health()` + `list_models(false)`, forces `models` empty if unreachable), `format_router_models`/
  `router_model_status_label`/`router_model_busy`/`router_health_label` (pure), `format_hf_results`
  (pure, calls `gguf_quants`), `format_auth_entries` (pure), `format_ollama_panel` (pure) — all
  covered by `models_panel_tests`/`auth_panel_tests` (backend.rs lines ~2505-2718), pure-function
  level only (no test drives the impure orchestration end-to-end; that's only manually/
  integration-verified per the source's own comments, e.g. "not verifiable on this dev machine").
- **`UiCmd`'s 9 local-model variants** (`OpenModels`, `ServeRapidMlxModel(String)`,
  `LoadRouterModel(String)`, `UnloadRouterModel(String)`, `SearchHfModels(String)`,
  `DownloadRouterModel(String)`, `AddOllamaToPi`, `ServerDotClicked`, `SaveApiKey{provider,key}`)
  dispatch to handler functions in `backend.rs` (`open_models_panel` 1812, `serve_rapid_mlx_model`
  2726, `load_router_model`/`unload_router_model`/`download_router_model` 1969-2031 sharing a
  `poll_router_until_idle` loop — 500ms ticks, 120s bound, stops early once no row is busy —
  `search_hf_models` 2048, `add_ollama_to_pi` 1923, `save_api_key` 1872). Every one of these is
  `Transcript`/`UiSink`-coupled (calls `transcript.note(...)`, `transcript.ui.set_*_panel(...)`) —
  not directly reusable by `pi-core-ffi`, same as every other `UiCmd` handler ported in SW1-SW3; the
  *sequence* of `local::*`/panel-formatter calls each one makes is the reusable reference behavior.
  `serve_rapid_mlx_model` is the one handler that touches `client` directly
  (`client.set_model("rapid-mlx", alias)`, after stopping any previously-managed server and waiting
  up to 180s for the new one's `Ready:` marker) — every other handler here never touches `PiClient`
  except for the `refresh_models` "nudge pi's own model picker" call some of them make afterward,
  which SW4 doesn't need to replicate (no picker exists in Swift yet — see Goal).
- **Panel refresh is action-triggered, never a background timer** (confirmed: only the status-bar
  server dot polls every 5s, and that's explicitly out of scope). `open_models_panel` is the one
  call that populates all 5 setters (`set_rapid_mlx_panel`, `set_router_panel`, `set_ollama_panel`,
  `set_auth_entries`, `show_models_panel`) at once; `set_rapid_mlx_panel`/`set_router_panel` are
  **deliberately separate setters** (doc comment, backend.rs lines 169-176) so the router's
  post-action poll loop doesn't repeat rapid-mlx's expensive multi-shellout scan on every 500ms
  tick — any FFI/Swift design must preserve this separation, not merge them into one combined
  refresh call.
- **`pi-core-ffi`'s current state confirms zero existing local-model surface** (grep across
  `pi-core-ffi/src`/`apps/pi-mac` for rapid-mlx/ollama/router/api-key/server-dot: zero matches);
  `PiSession::new_demo`'s doc comment explicitly calls this "SW4+ scope" — the deferral this
  milestone resolves. `pi-core-ffi/Cargo.toml` has no `reqwest`/`sysinfo`/`directories` today (those
  are `pi-core`-only); `session_index.rs`'s `SessionIndex` (stateless `#[derive(uniffi::Object,
  Default)]`, `#[uniffi::export(async_runtime = "tokio")]`, no owned `tokio::runtime::Runtime`,
  path-parameterized private helpers for testability against fixture dirs) is the template a new
  stateless local-model FFI object should follow.
- **`apps/pi-mac/PiMac/`**: `SidebarView.swift`'s `NSOpenPanel`-driven `pickProject()` (calls
  `Task { await model.switchProject(...) }` from a toolbar `Button`) is the established
  "toolbar action opens something, drives an async `AppModel` call" pattern to follow for a new
  "Models" entry point. No command palette or keyboard-shortcut system exists yet in the Swift app
  (Slint's Cmd+M/palette entry points don't have a Swift equivalent to hook into) — SW4 adds a
  plain toolbar button (optionally with a bare `.keyboardShortcut` modifier, not a palette).

### Design

- **New crate `crates/pi-local`**, `git mv` of `pi-core/src/local/*` verbatim (auth_json/hf/
  models_json/ollama/rapid_mlx/router/system_fit — no logic changes), plus a new `pi_local::panel`
  module holding the pure/impure "collect + format" pipeline moved out of `pi-core/src/backend.rs`:
  `RapidMlxSnapshot`, `RapidMlxPanelData`, `RouterPanelData`, `collect_rapid_mlx_snapshot`,
  `format_rapid_mlx_panel`, `fetch_router_state`, `format_router_models`,
  `router_model_status_label`, `router_model_busy`, `router_health_label`, `format_hf_results`,
  `format_auth_entries`, `format_ollama_panel` (their existing tests move with them). `pi-core`
  depends on `pi-local`, replaces `pub mod local;` with `pub use pi_local as local;` in
  `src/lib.rs` (same light-touch re-export `pi-render`'s `highlight`/`segmenter` used in SW3, so
  every `crate::local::...`/`local::...` call site throughout `backend.rs` keeps resolving
  unchanged), and its `open_models_panel`/`serve_rapid_mlx_model`/`load_router_model`/etc. call
  `pi_local::panel::*` instead of local functions — zero behavior change. `classify_rapid_mlx_dot`/
  `compute_server_dot`/the `SERVER_DOT_*` constants/the 5s-timer wiring in `run_session` all stay
  in `pi-core` untouched (server dot is out of scope). `pi-core`'s `Cargo.toml` drops `sysinfo`/
  `directories`/`reqwest` (now only reached transitively via `pi-local`), mirroring exactly how
  SW3's `pi-render` extraction dropped `pulldown-cmark`/`syntect`.
- **`pi-core-ffi` adds a `pi-local` dependency** and a new `local_models.rs` module: UniFFI
  `Record` mirrors + `From` conversions (same pattern as `row.rs`'s `RowRecord: From<pi_render::
  RowSpec>`) for `RapidMlxPanelRecord{version, running_summary, cached: Vec<CachedModelRecord>,
  catalog_count: u32}`, `CachedModelRecord{alias, hf_repo, size, fit_label}`,
  `RouterPanelRecord{status_label, base_url, models: Vec<RouterModelRecord>}`,
  `RouterModelRecord{id, status_label, loaded, busy}`, `HfResultRecord{id, gated, downloads: i32,
  quants: Vec<String>}`, `OllamaPanelRecord{detected, summary, model_count: i32}`. Auth entries stay
  `Vec<String>` (already pre-formatted labels — no Record needed).
- **New stateless `LocalModelIndex` object**, following `SessionIndex`'s exact shape (no owned
  runtime, `#[uniffi::export(async_runtime = "tokio")]`, fresh `LlamaRouter::default()`/
  `HfSearch::default()`/`OllamaProbe::default()` per call — matching the reference implementation's
  own per-call-construction pattern, no connection pooling needed for infrequent interactive
  actions):
  ```rust
  #[derive(uniffi::Object, Default)]
  pub struct LocalModelIndex;

  #[uniffi::export(async_runtime = "tokio")]
  impl LocalModelIndex {
      #[uniffi::constructor]
      pub fn new() -> Self { Self::default() }

      pub async fn refresh_rapid_mlx_panel(&self) -> RapidMlxPanelRecord { ... }
      pub async fn refresh_router_panel(&self) -> RouterPanelRecord { ... }   // kept separate from
      pub async fn refresh_ollama_panel(&self) -> OllamaPanelRecord { ... }   // rapid-mlx's refresh —
      pub async fn refresh_auth_entries(&self) -> Vec<String> { ... }         // see Verified facts

      pub async fn search_hf_models(&self, query: String) -> Result<Vec<HfResultRecord>, LocalModelError>;
      pub async fn start_load_router_model(&self, id: String) -> Result<(), LocalModelError>;
      pub async fn start_unload_router_model(&self, id: String) -> Result<(), LocalModelError>;
      pub async fn start_download_router_model(&self, model: String) -> Result<(), LocalModelError>;
      pub async fn add_ollama_to_pi(&self) -> Result<(), LocalModelError>;
      pub async fn save_api_key(&self, provider: String, key: String) -> Result<(), LocalModelError>;
  }
  ```
  Each method's body is a direct, thin call into `pi_local::{router,hf,ollama,auth_json,
  models_json,system_fit}::*` + `pi_local::panel::*`, mirroring the reference handler's sequence
  minus the `transcript.note`/`UiSink` push (replaced by `Result`/return value) and minus the
  `refresh_models` "nudge pi's picker" step (no picker to nudge — see Goal). `save_api_key`/
  `add_ollama_to_pi` split a path-parameterized private helper out from the public method
  (`save_api_key_at(path: &Path, ...)`, mirroring `session_index.rs`'s `sessions_at(root: &Path,
  ...)` split) so they're unit-testable against a tmpdir instead of the real `~/.pi/agent/`.
- **`start_load_router_model`/`start_unload_router_model`/`start_download_router_model` are
  deliberately one-shot** (fire the single POST via `LlamaRouter::{load_model,unload_model,
  download_model}` and return immediately) **rather than porting `poll_router_until_idle`'s
  blocking 500ms/120s loop across FFI.** A UniFFI async call blocking for up to 2 minutes is an
  awkward fit for Swift's structured concurrency and gives the caller no way to show live progress
  mid-poll; instead, Swift owns the polling loop itself (a `Task` calling
  `refreshRouterPanel()` every 500ms, stopping once no row is `busy` or after the same 120s bound),
  reusing the *outcome* of `poll_router_until_idle` (rows transition busy → loaded/failed within
  the same window) via the already-established pull-based-refresh pattern `AppModel` uses for
  sessions — not a strict single-function port, an intentional adaptation to a request/response FFI
  shape.
- **`ServeRapidMlxModel` is the one action that must live on `PiSession`, not `LocalModelIndex`** —
  it calls `client.set_model("rapid-mlx", alias)` on the *same* live `PiClient` that `PiSession`'s
  `run()` loop owns, and needs to track a managed-server child (`Option<ManagedServer>`) across
  calls the same way `client`/`events` are already tracked as `run()`-scoped mutable locals. New
  `ChatCmd::ServeRapidMlxModel{alias, reply}` + `PiSession::serve_rapid_mlx_model(alias) ->
  Result<(), PiSessionError>` (same `oneshot`-reply pattern as `switch_session`). `run()` gains a
  `managed_rapid_mlx: Option<Managed>` local (`struct Managed { alias: String, server:
  pi_local::rapid_mlx::ManagedServer }`, mirroring `pi-core`'s own `ManagedRapidMlx`); the new arm
  stops any previous managed server, spawns+waits-ready via `pi_local::rapid_mlx::ManagedServer`,
  then calls `client.set_model(...)` — a small (~15-20 line), deliberately duplicated copy of
  `pi-core`'s `serve_rapid_mlx_model` sequence, matching this crate's established "doesn't depend
  on `pi-core`, small orchestration helpers reimplemented locally" posture (e.g. `active_session_path`/
  `chunks()` already work this way). `run_demo` gets a matching `ChatCmd::ServeRapidMlxModel` arm in
  `reply_demo_action` (reply `Ok(())` immediately) so demo mode doesn't look broken for this action.
- **SwiftUI**: `AppModel` gains `private let localModels = LocalModelIndex()` plus published panel
  state (`rapidMlxPanel`, `routerPanel`, `ollamaPanel`, `authEntries`, `hfResults`) and methods
  (`refreshModelsPanel()` — concurrent `async let` over all 4 refreshes; `serveRapidMlx(alias:)` —
  calls `session?.serveRapidMlxModel`, then `refreshModelsPanel()`; `loadRouterModel`/
  `unloadRouterModel`/`downloadHfModel` — fire the one-shot `start_*` call, then a bounded
  poll-`refreshRouterPanel()`-until-idle `Task`; `searchHfModels`; `addOllamaToPi`; `saveApiKey`) —
  the single source of truth `ModelsPanelView`/`HfSearchView` read from and act through, same role
  `AppModel` already plays for sessions. New `ModelsPanelView.swift` (rapid-mlx/router/ollama/auth
  sections, mirroring `ModelsOverlay`'s four blocks) and `HfSearchView.swift` (Enter-to-search field,
  results with gated-license link via `NSWorkspace.shared.open`, quant chips triggering download),
  presented as a `.sheet` from a new toolbar button (mirroring `SidebarView`'s "Switch Project"
  button) with a `.keyboardShortcut("m")`.

### Work breakdown

1. Extract `crates/pi-local`: `git mv` the 7 `local/*` files + `mod.rs`→`lib.rs`; new `Cargo.toml`
   (`reqwest`/`sysinfo`/`directories`/`serde`/`serde_json`/`thiserror`/`tokio`/`tracing` — confirm
   exact set against each file's actual `use`s while moving). `pi-core` depends on it, `pub use
   pi_local as local;` re-export. Zero behavior change — `cargo test -p pi-core` green, 47 moved
   tests pass verbatim.
2. Move the panel-formatting layer (`RapidMlxSnapshot`/`RapidMlxPanelData`/`RouterPanelData`/
   `collect_rapid_mlx_snapshot`/`format_rapid_mlx_panel`/`fetch_router_state`/`format_router_models`/
   `router_model_status_label`/`router_model_busy`/`router_health_label`/`format_hf_results`/
   `format_auth_entries`/`format_ollama_panel`) into `pi_local::panel`; `pi-core`'s handlers call it
   instead. `models_panel_tests`/relevant `auth_panel_tests` cases move with it (leave
   `classify_rapid_mlx_dot`/server-dot tests in `pi-core`). Zero behavior change.
3. `pi-core/Cargo.toml`: drop `sysinfo`/`directories`/`reqwest`. `pi-core-ffi/Cargo.toml`: add
   `pi-local`.
4. `pi-core-ffi`: new `local_models.rs` — the 6 UniFFI Records + `From` conversions; unit-test the
   conversions directly (no FFI needed).
5. `LocalModelIndex` object: the 4 refresh methods + `search_hf_models`/`start_load_router_model`/
   `start_unload_router_model`/`start_download_router_model`/`add_ollama_to_pi`/`save_api_key`;
   `LocalModelError`; path-parameterized test helpers + unit tests against tmpdirs for
   `save_api_key`/`add_ollama_to_pi`.
6. `PiSession`: `ChatCmd::ServeRapidMlxModel` + `serve_rapid_mlx_model` async method; `run()`'s
   `managed_rapid_mlx: Option<Managed>` local + the stop/spawn/wait-ready/`set_model` arm;
   `run_demo`'s matching no-op reply arm.
7. Regenerate bindings (`scripts/build-rust.sh`); `swiftc -typecheck` against the generated bindings
   to confirm the new Swift API shape before touching Xcode/SwiftUI code.
8. SwiftUI: `AppModel`'s `localModels` field, published panel state, and the 8 new async methods
   (including the Swift-side bounded router-polling `Task`).
9. `ModelsPanelView.swift` (4 sections) + `HfSearchView.swift`; toolbar button + `.sheet` wiring;
   `project.pbxproj` entries for the 2 new files.
10. Verification: extend `examples/spike_check.rs` with a `models` mode — at minimum a real
    `search_hf_models` call against the live Hugging Face API (mirroring `local::hf::tests::
    live_search_round_trips_against_the_real_api`'s existing convention), plus rapid-mlx/router/
    Ollama actions attempted-but-tolerant-of-not-installed (matching this project's established
    "skip gracefully when the external tool isn't present" pattern, e.g. `pi-rpc`'s own tests).
    Full `cargo fmt/clippy/test --workspace`; real `xcodebuild -scheme PiMac build`; manual pass —
    open the Models panel, confirm every section renders a sensible state (detected/not, populated/
    empty) for whatever's actually installed on the dev machine; live HF search returns real
    results with correct quant chips; saving an API key writes `~/.pi/agent/auth.json` (0600) and
    the masked entry appears.

### Risks

- **Managed rapid-mlx server orchestration is duplicated, not shared**, between `pi-core` and
  `pi-core-ffi` (~15-20 lines) — deliberate, matching this crate's established posture of small
  reimplemented helpers rather than a `pi-core` dependency; low risk given the sequence is short and
  well-documented in Verified facts.
- **Swift-side polling is a UX adaptation, not a literal port** of `poll_router_until_idle` — verify
  manually that a load/unload/download actually settles (row transitions from busy to loaded/failed)
  within the mirrored 500ms/120s cadence; if it feels sluggish, tightening the interval is a
  same-shape fix, not a redesign.
- **rapid-mlx/router/Ollama are unlikely to be installed on most dev machines** (this one included,
  per SW0-era coverage notes) — most manual verification will exercise the "gracefully renders
  empty/undetected" paths rather than full serve/load/download flows; call this out explicitly
  rather than treating a clean run on an un-instrumented machine as full coverage.
- **No composer model picker, no server dot** — explicit scope cut (Goal), not an oversight; flagged
  again here so it isn't mistaken for a gap discovered late.

### Acceptance criteria

- `cargo fmt --check && cargo clippy --workspace && cargo test --workspace` green; `pi-local`'s
  moved tests (47) plus the relocated panel-formatter tests pass unchanged, proving zero behavior
  change to the Slint app's models panel.
- Real verification: the Swift Models panel shows the same rapid-mlx/router/Ollama/auth state the
  Slint app would show on the same machine; a live Hugging Face search returns real results with
  correctly-parsed quant chips; saving an API key round-trips through `~/.pi/agent/auth.json`
  (0600) and the masked entry appears in the panel; wherever rapid-mlx/router/Ollama are actually
  installed and running, serve/load/unload/download/add-to-pi behave as designed.
- `apps/pi-mac` builds clean via `xcodebuild -scheme PiMac build` from a clean checkout, same bar as
  SW1-SW3.

---

## Fix — App diagnostics: log failures via macOS unified logging

**Context:** asked directly (not part of the SW-numbered sequence) — "is there any logging if
things go wrong, like pi not being found or failing to start?" — and scheduled to land before SW5.
Audited the answer directly against the code: **no.** `AppModel.statusMessage`, rendered as a
single-line red caption in `ChatView`'s status bar (`ChatView.swift:67-85`, the only place it's
ever read), is the *entire* signal a user of the compiled `PiMac.app` ever sees. Nothing reaches
Console.app, stderr, or a file: `pi-core-ffi/Cargo.toml` only pulls in `tracing-subscriber` as a
dev-dependency (used solely by `examples/spike_check.rs`'s own standalone `main()`), so no
subscriber is ever installed inside the shipped library — every `tracing::debug!`/`warn!` call
already in `pi-rpc/src/client.rs` (including one that forwards `pi`'s own child-process stderr,
line 110 — exactly the detail you'd want when `pi` fails to start) is a silent no-op in the real
app. There's no `os_log`/`Logger`/`print`/`NSLog` anywhere in the Swift app either. And
`ensure_usable_path`/`login_shell_path` (`lib.rs:708-724`/`756-773`, the PATH-repair logic for
Finder/Dock-launched apps) is completely silent on failure — if the login-shell PATH lookup times
out or fails, it just gives up with zero indication that was even attempted.

### Design

- **Install a real subscriber, once, inside the app.** Add `tracing-oslog` (crates.io, Zlib
  license, `~313k` downloads, a `tracing_subscriber::Layer` wrapping Apple's unified logging —
  verified via its docs.rs page) as a normal (non-dev) dependency of `pi-core-ffi`. New
  `fn ensure_logging_initialized()`:
  ```rust
  fn ensure_logging_initialized() {
      use tracing_subscriber::prelude::*;
      static INIT: std::sync::Once = std::sync::Once::new();
      INIT.call_once(|| {
          let _ = tracing_subscriber::registry()
              .with(tracing_oslog::OsLogger::new("dev.slinty-pi.pi-mac"))
              .try_init(); // discard Err: harmless if a subscriber is already set (see below)
      });
  }
  ```
  Subsystem string matches `AppModel`'s existing `"dev.slinty-pi.pi-mac.*"` `UserDefaults` key
  prefix. Called at the top of `PiSession::new`, `PiSession::new_demo`, and `LocalModelIndex::new`
  — cheap after the first call (the `Once` guard), and covers every entry point regardless of
  which one Swift happens to construct first.
- **Must not panic or regress `spike_check.rs`.** `tracing_subscriber`'s global default can only be
  set once process-wide; `spike_check.rs`'s own `main()` already calls `tracing_subscriber::fmt()
  ...init()` *before* constructing a `PiSession` — so `ensure_logging_initialized`'s own
  `try_init()` call will find one already set and return `Err`, which is deliberately discarded
  (`let _ = ...`), not `unwrap`ed. Net effect: `spike_check` keeps its existing stderr-`fmt` output
  unchanged (no regression to that dev tool); the real `PiMac.app`, where nothing has installed a
  subscriber yet, gets `tracing-oslog` as the first and only one.
- **Close the three specific silent gaps found**, each a one-or-two-line addition at an existing
  call site: `PiSession::new`'s two `PiSessionError::Spawn` constructions (`lib.rs:149`, `157`) —
  `tracing::error!` the full message immediately before returning `Err`, so it's captured even
  though the UI only ever shows a generic "Could not start pi: …" line; `run()`'s `"pi exited"`
  branch (`lib.rs:473`); `login_shell_path` returning `None` (`lib.rs:756-773`) — `tracing::warn!`
  when the login-shell PATH lookup itself fails/times out (currently zero signal that a repair was
  even attempted), and `tracing::debug!` on success for confirmation during diagnosis.
- **Centralize `ChatSink::on_error` so every push-based chat/session error is logged, not just the
  three above.** 7 call sites in `lib.rs` currently call `sink.on_error(...)` directly (session
  restore/switch failures, `PiClient::prompt`/`abort` errors, `"pi exited"`, hydration failures,
  non-aborted model errors). Replace with one helper:
  ```rust
  fn report_error(sink: &dyn ChatSink, message: String) {
      tracing::error!("{message}");
      sink.on_error(message);
  }
  ```
  and call `report_error(sink.as_ref(), ...)` (in `run()`, which holds `Arc<dyn ChatSink>`) or
  `report_error(sink, ...)` (in `hydrate_and_push`/`apply`, which already take `&dyn ChatSink`)
  everywhere `sink.on_error(...)` is called today. This guarantees the invariant "everything ever
  pushed to `ChatSink::on_error`" — the primary channel `on_error` was designed for — is always
  logged, for free, for any future call site too.
- **Explicitly out of scope, to keep this a tight fix**: the `Result`-returning session-lifecycle
  (`switch_project`/`new_session`/.../`serve_rapid_mlx_model`) and `LocalModelIndex` action methods
  already surface their errors to Swift today (each has its own `catch { statusMessage = ... }` in
  `AppModel.swift`) — they're not *silent* the way `on_error`/spawn-failure/path-repair are, just
  not *logged*. Covered instead, uniformly and cheaply, by the Swift-side change below rather than
  auditing every `reply.send(Err(...))` site in Rust.
- **Swift side: log everything `statusMessage` ever shows, at the one place it's set.** `AppModel`
  gains `import os` and a private `Logger` (`Logger(subsystem: "dev.slinty-pi.pi-mac", category:
  "app")`), plus a small `private func setStatus(_ message: String)` that does `logger.error(
  "\(message)")` then `statusMessage = message`. Every one of `AppModel.swift`'s ~10 existing
  `statusMessage = "..."` assignments (in `start`'s catch, `onError`, and every action's own
  `catch` block) becomes `setStatus("...")`. This is what closes the gap for the actions
  deliberately left untouched on the Rust side above — whatever eventually reaches
  `statusMessage`, from any origin, is now always in Console.app too, not just a one-line caption
  that gets overwritten by the next event.
- **Verify no Xcode linking regression** (SW4 already hit this once with `sysinfo`/OpenDirectory):
  `tracing-oslog` has a `cc` build-dependency (compiles a small C shim against Apple's unified
  logging headers) — confirm a clean `xcodebuild -scheme PiMac build` still succeeds after adding
  it, before considering this done.

### Work breakdown

1. `pi-core-ffi/Cargo.toml`: move `tracing-subscriber` question aside — add `tracing-oslog` as a
   real dependency (`tracing-subscriber` itself already isn't a runtime dep; `tracing-oslog` pulls
   in `tracing-core`/`tracing-subscriber` transitively as needed for the `Layer`/`registry()` glue).
   `ensure_logging_initialized()`; call it from `PiSession::new`, `PiSession::new_demo`,
   `LocalModelIndex::new`.
2. Add the three targeted `tracing::error!`/`warn!`/`debug!` calls (spawn failure ×2, pi-exited,
   login-shell-path success/failure).
3. `report_error` helper; replace all 7 `sink.on_error(...)` call sites in `lib.rs` with it.
4. `AppModel.swift`: `import os`, private `Logger`, `setStatus(_:)`; replace every
   `statusMessage = "..."` assignment with `setStatus("...")`.
5. Regenerate bindings if the Rust-side public API shape changed at all (it shouldn't — this is
   internal-only, no new `#[uniffi::export]` surface); rebuild the xcframework regardless since the
   staticlib's contents changed.
6. Verification (see below).

### Risks

- **`tracing-oslog`'s exact `tracing::Level` → `OSLogType` mapping** wasn't confirmed in detail
  during planning (only that the crate exists, is real, and wraps unified logging correctly) — pin
  down during implementation whether `error!`/`warn!` land at a level `log show`/Console.app
  display *by default* (unified logging can silently drop low-severity messages from the retained
  archive); adjust the levels used in step 2/3 if `error!`/`warn!` turn out to need bumping.
- **`cc` build-dependency** — low risk (a tiny, well-established shim, not a large transitive
  dependency graph like `sysinfo` was), but explicitly verified via a real `xcodebuild` pass per
  the Design section's last bullet, not assumed safe by analogy alone.
- **Scope discipline**: explicitly not logging session-lifecycle/local-model action errors on the
  Rust side (see Design) — if that turns out to matter in practice, it's a small, same-shaped
  follow-up (more `report_error`-style call sites), not a redesign.

### Acceptance criteria

- `cargo fmt --check && cargo clippy --workspace && cargo test --workspace` green.
- `xcodebuild -project PiMac.xcodeproj -scheme PiMac build` succeeds from a clean
  `Generated`/`PiCoreFFI.xcframework` rebuild.
- Manual real-machine check: temporarily break `pi`'s discoverability (e.g. rename the binary or
  point `cwd` somewhere `pi` can't run), launch the app, and confirm the failure is visible in
  `log stream --predicate 'subsystem == "dev.slinty-pi.pi-mac"'` (or Console.app filtered to that
  subsystem) with a real, specific message — not just the existing one-line red status caption.
  Repeat for a live session that gets killed mid-conversation (`"pi exited"`).

---

## Fix — Live markdown/code rendering while a reply is streaming

**Context:** asked directly — "is it normal for the stream text to not be completely formatted
(raw markdown) until streaming has completed?" Confirmed: yes, and it's the exact, explicitly-named
scope cut from the SW3 plan's Design section ("no live 'code block formats as it streams' parity
with Slint's 33ms flush... a deliberate, intentional scope reduction"). The user asked to plan
closing that gap next.

### Verified facts

- **Slint's live re-segmentation is event-gated, not a background timer.** `pi-core::backend::
  Transcript::flush_stream`/`apply_delta` (`crates/pi-core/src/backend.rs:316-346`, `410-473`) only
  re-segments inside the `TextDelta` handler, and only if `region.last_flush.elapsed() >=
  TEXT_FLUSH` (`33ms`, line 42) — a burst of deltas within that window just accumulates into
  `region.buffer`; the first delta after `TextStart` always flushes immediately (`last_flush`
  seeded to `Instant::now() - TEXT_FLUSH`). Each flush re-runs `segment_markdown` on the *entire*
  growing buffer, diffs the fresh `Vec<Segment>` against `region.prev` index-by-index (`Segment`
  derives `PartialEq`), and for each changed/new index either `ui.set(index, spec)` (row already
  exists) or `push_row(spec)` (new row); if the segment count *shrinks* between flushes it also
  `ui.truncate`s the stale trailing rows. This whole diff/set/truncate dance exists because Slint's
  `Ui` is an imperative, addressable row model — SwiftUI's `ForEach` over a plain array doesn't need
  the same machinery (see Design).
- **The building blocks are already `pub` in `pi-render` and need no changes**:
  `pi_render::segmenter::segment_markdown(source: &str) -> Vec<Segment>` (module-qualified) and
  `pi_render::spec_for_segment(segment: &Segment, first: bool, dark: bool, raw: &str) -> RowSpec`
  (root re-export) — the exact pair `Transcript::flush_stream` already calls. `pi-core-ffi` already
  depends on `pi-render` (since SW3) and already has `RowRecord: From<pi_render::RowSpec>`
  (`crates/pi-core-ffi/src/row.rs`).
- **`pi-core-ffi` currently ports none of this.** `apply()` (`lib.rs:707-722`) only forwards raw
  `AssistantMessageEvent::TextDelta` text via `ChatSink::on_text_delta(delta: String)`; `run()`'s
  event arm has an explicit code comment (`lib.rs:515-518`) recording that porting `Transcript`'s
  incremental live-flush machinery across FFI was a deliberate SW3 omission. `ChatSink` has no
  per-delta structured-row callback — `on_history_replaced(rows: Vec<RowRecord>)` is the only
  row-bearing method, fired only post-settle (`hydrate_and_push`) or on a session-changing reset.
- **Swift side today**: `AppModel.onTextDelta` appends every delta to one flat `transcript: String`
  (`AppModel.swift`); `ChatView` renders `model.rows` (from `onHistoryReplaced`) followed
  unconditionally by one raw `Text(model.transcript)` — no markdown/`AttributedString` parsing at
  all. `onHistoryReplaced` is the only place `transcript` is cleared. `send()` also prepends the
  user's own `"> {prompt}"` text into this same accumulator before the reply starts streaming —
  pre-existing SW1 behavior, out of scope to change here.

### Design

- **Swift pulls; Rust doesn't push, and `run()`/`apply()`/`ChatSink` stay untouched.** Rather than
  porting `Transcript`'s `StreamRegion`/diff/set/truncate state machine into `run()` (a new
  `ChatSink` method, new mutable loop state, and changes to already-working, tested code), add one
  new, fully additive, stateless method: `PiSession.previewRows(markdown: String) async ->
  [RowRecord]`. It doesn't touch `client`/`events`/the command channel at all — just
  `segment_markdown` + `spec_for_segment` over whatever text Swift hands it, using the same
  `dark: Arc<AtomicBool>` field `PiSession` already tracks (updated via `set_dark_mode`, so no new
  param needed). Delegates to a free, pure, directly unit-testable function so the UniFFI method
  itself is a one-liner:
  ```rust
  fn preview_rows(markdown: &str, dark: bool) -> Vec<RowRecord> {
      pi_render::segmenter::segment_markdown(markdown)
          .iter()
          .enumerate()
          .map(|(i, seg)| RowRecord::from(pi_render::spec_for_segment(seg, i == 0, dark, markdown)))
          .collect()
  }
  ```
  Works identically in demo mode (`PiSession.newDemo`) with no special-casing, since `dark` is a
  plain struct field set by both constructors.
- **SwiftUI doesn't need Slint's index-aligned `set`/`truncate` dance.** Each throttled call fully
  replaces a new `streamingRows: [RowRecord]` array (separate from the authoritative `rows`, kept
  doc-contract-clean per `on_history_replaced`'s existing "replaces the *entire* transcript" meaning
  — reusing it for partial live previews would conflate the two). `ChatView`'s `ForEach(Array(
  streamingRows.enumerated()), id: \.offset)` (same pattern already used for `rows`) naturally
  handles a shrinking segment count on the next replace — no explicit truncate needed, SwiftUI just
  renders a shorter array.
- **Swift owns the throttle, mirroring `TEXT_FLUSH`'s cadence and gating, not a bare interval
  timer.** `AppModel` tracks `lastPreviewFlush` and a single in-flight `pendingPreviewTask:
  Task<Void, Never>?`. `onTextDelta` appends to `transcript` (unchanged) then calls a
  `scheduleStreamPreview()` helper: if a preview refresh is already scheduled/in-flight, do nothing
  (coalesces bursts, same effect as Slint's `region.last_flush.elapsed() < TEXT_FLUSH` early-return);
  otherwise schedule one after `max(0, 33ms - time since last flush)`, so a delta arriving right
  after a flush waits out the rest of the window and a delta arriving after a long gap fires
  immediately — the same two behaviors `flush_stream`'s gate produces, just computed Swift-side.
  `onTurnEnd`/`onHistoryReplaced` clear `streamingRows` (mirroring how `transcript` is already
  cleared only in `onHistoryReplaced`).
- **`transcript` becomes purely a private accumulator.** Once `ChatView` renders `streamingRows`
  instead of raw `transcript` text, nothing outside `AppModel` needs to read it — tighten it from
  `private(set)` to `private`.
- **Thinking/tool-call live visibility is explicitly out of scope for this fix.** `apply()`'s
  `ThinkingStart/Delta/End` and `ToolExecutionStart/Update/End` arms still fall into `_ => {}` and
  stay invisible to Swift entirely (not just unformatted — genuinely never forwarded). That's a
  separate, larger gap (would need new `ChatSink` surface, not just a preview-rendering trick) and
  isn't what was asked; noted here so it isn't mistaken for something this fix silently covers.

### Work breakdown

1. `pi-core-ffi`: free `preview_rows(markdown: &str, dark: bool) -> Vec<RowRecord>` function +
   `PiSession::preview_rows(markdown: String) -> Vec<RowRecord>` (`#[uniffi::export(async_runtime =
   "tokio")]`, same block as `switch_project`/etc.) delegating to it. Unit tests against the free
   function directly (e.g. an unclosed code fence still segments as a `"code"` row mid-stream,
   mirroring `segmenter.rs`'s own `unclosed_fence_streams_as_code` test).
2. Regenerate bindings; `swiftc -typecheck` to confirm the new `previewRows` method's Swift shape.
3. `AppModel`: `streamingRows: [RowRecord]` (published), `lastPreviewFlush`/`pendingPreviewTask`
   private state, `scheduleStreamPreview()`/`refreshStreamPreview()`; wire into `onTextDelta`; clear
   `streamingRows` in `onTurnEnd`/`onHistoryReplaced`; tighten `transcript` to `private`.
4. `ChatView`: replace the `if !model.transcript.isEmpty { Text(model.transcript)... }` block with
   a `ForEach` over `model.streamingRows` using `RowView` (same call shape as the existing `model.
   rows` loop just above it).
5. Verification (below).

### Risks

- **UniFFI round-trip cost at ~30Hz for a long streamed reply** — each call re-serializes the whole
  growing buffer and re-runs `spec_for_segment`'s syntect highlighting for any code segment from
  scratch (the same per-flush cost Slint already pays today, just now paid by Swift too, so not a
  *new* cost profile — but worth watching on a very long reply). If it's ever visibly janky, a
  cheap same-shaped mitigation is skipping preview calls below a minimum buffer-growth threshold,
  not a redesign.
- **The user's own `"> {prompt}"` echo text shares `transcript` with the assistant's reply buffer**
  (pre-existing SW1 behavior) — `segment_markdown` will parse a leading `> ` as a real Markdown
  blockquote in the live preview until the turn settles and the real `"user"`-kind row replaces it.
  Cosmetically harmless (arguably a mild visual win, distinguishing the echoed prompt), not
  something this fix changes.
- **Swift's throttle is a close, not exact, mirror of `TEXT_FLUSH`'s gating** — functionally
  equivalent (same coalesce-bursts-then-flush-at-window-boundary shape, same 33ms target), not
  byte-for-byte identical timing to the Rust implementation. Acceptable; flagged so a minor
  cadence difference isn't mistaken for a bug later.

### Acceptance criteria

- `cargo fmt --check && cargo clippy --workspace && cargo test --workspace` green; the new
  `preview_rows` unit tests pass.
- `xcodebuild -project PiMac.xcodeproj -scheme PiMac build` succeeds from a clean
  `Generated`/`PiCoreFFI.xcframework` rebuild.
- Real `pi --mode rpc` verification: send a prompt whose reply includes a heading, a code block,
  and some bold/italic text; confirm all three render richly (not raw markdown) *while the reply is
  still streaming*, updating roughly every 33ms as more text arrives, and that the transition to
  the finalized `onHistoryReplaced` rows once the turn settles is visually seamless (no flash of
  raw text or duplicated content).

---

## Milestone SW5 — Extension-UI dialogs & permission gating in Swift

**Goal:** answer pi's `select`/`confirm`/`input`/`editor` extension-UI dialogs from the SwiftUI
app via native `.alert`/`.sheet` presentation, so pi extensions that gate on these (most notably
tool-call permission prompts) work against the Swift app instead of hanging indefinitely. This is
greenfield work, not a port — see Verified facts.

### Verified facts

- **The wire protocol is fully typed and ready to reuse, in `pi-rpc`, unchanged.**
  `ExtensionUiRequest` (`crates/pi-rpc/src/types.rs:181-201`): `{id, method, title?, message?,
  options?, placeholder?, prefill?, timeout?, #[serde(flatten)] rest}` — `method` is a raw string
  (`"select" | "confirm" | "input" | "editor" | "notify"`, plus untyped others below), not a typed
  enum. `Event::ExtensionUiRequest(ExtensionUiRequest)` and `Event::ExtensionError{extension_path,
  event, error}` (`types.rs:309-318`) are both existing `Event` variants. `ExtensionUiReply`
  (`types.rs:324-330`, `Value(String) | Confirmed(bool) | Cancelled`) is the reply shape.
  `PiClient::reply_extension_ui(&self, request_id: &str, reply: ExtensionUiReply) ->
  Result<(), PiError>` (`crates/pi-rpc/src/client.rs:218-240`) hand-writes
  `{"type":"extension_ui_response","id":...,"value"|"confirmed"|"cancelled":...}` straight to
  stdin — **fire-and-forget, no correlated response line comes back from pi** (not registered in
  `pending`, doesn't go through `PiClient::request`/`Command`). Both `ExtensionUiRequest` and
  `ExtensionUiReply` are already exported at `pi_rpc`'s crate root (`lib.rs:26-27`), so
  `pi-core-ffi` (which already depends on `pi-rpc`) needs no new dependency.
- **`pi-core-ffi` currently drops both events entirely.** `apply()` (`lib.rs:772-787`) matches
  `AgentStart`/`AgentSettled`/`TextDelta`/`MessageEnd`/non-aborted `Error` only; everything else,
  including `ExtensionUiRequest`/`ExtensionError`, falls into `_ => {}`. Concretely: today, any pi
  extension that issues a blocking `select`/`confirm`/`input`/`editor` request against the SwiftUI
  app hangs forever (nothing ever calls `reply_extension_ui`) until pi's own optional `timeout`
  elapses agent-side — worse than doing nothing, since the Slint app's stub at least auto-cancels
  (see next bullet). `ChatCmd` (`lib.rs:102-135`) has no reply-sending variant; `PiSession` has no
  method wrapping `reply_extension_ui`. This milestone is additive: no existing `ChatCmd` variant,
  `ChatSink` method, or `apply()` arm needs to change.
- **There is no working reference implementation anywhere in this codebase to port from.** Slint's
  own `handle_extension_ui` (`crates/pi-core/src/backend.rs:2732-2753`) unconditionally replies
  `Cancelled` to every `select`/`confirm`/`input`/`editor` request and posts an info row reading
  `"...auto-dismissed (dialogs land in M4)"` — a documented stub for the Slint app's own **not
  started** M4 milestone (`docs/plans/M4-agent-trust.md`, `docs/plans/README.md` status: planned).
  No `.slint` dialog component exists. SW5 is genuinely new work on both the Rust and Swift sides,
  not a port of an existing Slint feature (unlike every prior SWx milestone).
- **Permission gating is confirmed to not be a separate protocol** — `grep -rn "permission" -i`
  across `pi-rpc`/`pi-core`/`pi-core-ffi`/`slinty-pi` returns zero hits. Per
  `M4-agent-trust.md:15-17`: "pi has no built-in permission system by design; the sanctioned
  pattern is a `tool_call` extension that gates via `ctx.ui` dialogs." So "permission gating in
  Swift" (this milestone's name) means: render `confirm`/`select` dialogs well: a real gating
  extension is just a normal extension-UI consumer.
- **A real, already-installed gating extension exists on this dev machine** —
  `~/.pi/agent/extensions/pi-permission-system/` (its `config.json` gates `bash`/`write`/`edit`/
  `read`/`path`/`external_directory` with `ask`/`allow`/`deny` rules). Its own review log
  (`~/.pi/agent/extensions/pi-permission-system/logs/*-permission-review.jsonl`) shows real
  gated-request messages, e.g. `"Current agent requested bash command '...' (matched '*') (full
  command: '...'). Allow this command?"`, and confirms unanswered requests resolve
  `"resolution":"denied"` (a live confirmation of pi's documented agent-side `timeout` auto-deny
  behavior). A live probe against real `pi --mode rpc` in this session also captured the exact
  wire shape of this extension's **fire-and-forget** `setStatus` calls before being interrupted:
  `{"type":"extension_ui_request","id":...,"method":"setStatus","statusKey":"pi-permission-system"}`
  and `{...,"method":"setStatus","statusKey":"sandbox","statusText":"..."}` — confirming
  `M4-agent-trust.md`'s documented `setStatus{statusKey,statusText}` shape is real and, since
  those aren't named `ExtensionUiRequest` fields, lands only in `#[serde(flatten)] rest` today.
  **No live-observed evidence supports M4's hypothetical `"permission: bash"` title-prefix
  convention** — the real installed extension's messages carry the full human-readable request
  directly in `message`, with no confirmed structured title. Building bespoke "permission card"
  rendering keyed to an unconfirmed convention would be speculative; generic `confirm`/`select`
  rendering already serves this real extension correctly (see Design's explicit scope cut).
- **Swift-side precedent for both presentation styles already exists in this codebase**:
  `SidebarView.swift`'s rename action uses `.alert("Rename Session", isPresented:) { TextField(...);
  Button("Cancel", role: .cancel) {}; Button("Rename") {...} }` — the exact shape an `input`
  dialog needs. `SidebarView.swift`'s "Models" toolbar button uses `.sheet(isPresented:) {
  ModelsPanelView(model: model) }` — the shape `select`/`editor` need (more space than an alert
  comfortably holds). `ExtensionDialogRecord`'s natural `id: String` field lets it conform to
  `Identifiable` via a plain `extension ExtensionDialogRecord: Identifiable {}` for `.sheet(item:)`
  binding, the same way `SessionRecord`/`RowRecord` are consumed today (by `path`/array index, not
  `Identifiable`, but the mechanism is standard Swift and needs no source change on the generated
  type).

### Design

- **`pi-core-ffi`**: new `ExtensionDialogRecord` (mirrors `pi_rpc::ExtensionUiRequest` minus
  `rest` — `id, method, title, message, options, placeholder, prefill, timeout`) and
  `ExtensionDialogReply` (`uniffi::Enum`: `Value{value: String} | Confirmed{confirmed: bool} |
  Cancelled`, converting to `pi_rpc::ExtensionUiReply`) in a new `src/dialog.rs`, following
  `row.rs`/`local_models.rs`'s existing one-file-per-concern + `From` conversion pattern.
- **`ChatSink` gains one method**: `fn on_extension_dialog(&self, request: ExtensionDialogRecord)`,
  fired only for `method` in `{"select", "confirm", "input", "editor"}`. `apply()` gains a matching
  arm plus one for `Event::ExtensionError` (routed through the existing `report_error` helper, same
  as every other error path). **Explicit scope cut, called out so it isn't mistaken for an
  oversight**: `notify`/`setStatus`/`setWidget`/`setTitle`/`set_editor_text` (fire-and-forget,
  non-dialog signals — M4's broader, not-yet-built `DialogRouter`/toast/status-segment/widget
  surface) are matched separately and intentionally dropped, same as today. This milestone is named
  "dialogs & permission gating," not the fuller M4 surface.
- **Reply path is fire-and-forget, matching `send`/`abort`, not the oneshot-reply session-lifecycle
  actions** — because `reply_extension_ui` itself is a non-blocking stdin write with no correlated
  response, there's nothing meaningful to `await`. New `ChatCmd::ReplyExtensionDialog{request_id,
  reply}` (no oneshot sender); `run()` gains a match arm calling `client.reply_extension_ui(...)`,
  reporting failure via `report_error` on error; new plain (non-async) `PiSession::
  reply_extension_dialog(&self, request_id: String, reply: ExtensionDialogReply)` alongside
  `send`/`abort`. `run_demo`'s `reply_demo_action` gets a no-op arm (demo mode never emits
  dialogs, but must not fall through to its exhaustive-match `unreachable!`).
- **Swift owns a small FIFO dialog queue** (per `M4-agent-trust.md`'s "requests queue FIFO," the
  only part of that plan directly reused): `AppModel` gains `private(set) var pendingDialogs:
  [ExtensionDialogRecord] = []`, a `currentDialog` computed as `pendingDialogs.first`,
  `onExtensionDialog(request:)` appending to it, and `replyToCurrentDialog(_:)` popping the front
  and calling `session?.replyExtensionDialog(requestId:reply:)`. `abort()` is extended to
  proactively reply `.cancelled` to the current dialog (if any) and clear the queue before
  forwarding to `session?.abort()` — pi's own `tool_call`/dialog cancellation behavior on abort is
  unconfirmed (see Risks), so this is a defensive client-side safety net, not a protocol
  requirement. `onActiveSessionChanged`/`onHistoryReplaced` also clear `pendingDialogs` (no reply
  sent — the old client's request is presumed moot once its session context changes; see Risks).
- **New `ExtensionDialogView.swift`**: `extension ExtensionDialogRecord: Identifiable {}`, plus a
  view (attached to `ChatView` alongside the existing composer/status-bar chrome) presenting
  `confirm`/`input` via `.alert(_:isPresented:presenting:actions:message:)` and `select`/`editor`
  via `.sheet(item:)` — matching `SidebarView`'s existing alert-for-simple/sheet-for-complex split,
  and the milestone's own name ("native `NSAlert`/**sheet**" — plural, implying mixed
  presentation, not one uniform style). Per kind: `confirm` — message + "Deny"/"Allow" buttons
  (`Confirmed(false)`/`Confirmed(true)`; an Esc/tap-away dismiss replies `Cancelled`, distinct from
  an explicit "Deny"); `input` — `TextField` prefilled from `prefill`, "Cancel"/"Submit"
  (`Cancelled`/`Value`), exact shape of `SidebarView`'s existing rename alert; `select` — a `List`
  of `options`, tapping a row replies immediately (`Value`), plus a Cancel action; `editor` — a
  `TextEditor` prefilled from `prefill`, "Cancel"/"Submit" toolbar actions. An `.alert`'s
  `isPresented` binding's `set` closure treats any dismiss (not just an explicit button) as
  `Cancelled`, so Esc/tap-away never leaves a dialog silently stuck.
- **Explicit scope cuts** (flagged so none read as an oversight): no bespoke "permission card"
  rendering keyed to a title-prefix convention (no confirmed real convention exists — see Verified
  facts); no `notify`/`setStatus`/`setWidget`/`setTitle`/`set_editor_text` surfaces (M4's broader,
  unbuilt scope); no visual timeout countdown (pi auto-resolves the timeout agent-side regardless —
  M4 itself frames the countdown bar as "honesty, not logic"); no window/Dock attention-grabbing
  when the app is backgrounded while a dialog is pending (M4 §5 "Notifications & tray," unbuilt);
  no demo-mode synthetic dialog trigger (a real, already-installed gating extension gives a
  concrete, real verification path — see Verification below; a synthetic demo trigger can be added
  later if automated/no-`pi` testing of the dialog UI is ever needed).

### Work breakdown

1. `pi-core-ffi`: new `src/dialog.rs` (`ExtensionDialogRecord`, `ExtensionDialogReply` + `From`
   conversions); `ChatSink::on_extension_dialog`; `apply()`'s two new arms; `ChatCmd::
   ReplyExtensionDialog`; `run()`'s new match arm; `PiSession::reply_extension_dialog`;
   `reply_demo_action`'s no-op arm. Unit tests: `ExtensionDialogRecord`/`ExtensionDialogReply`
   conversions directly (no FFI needed), and an `apply()`-level test (mirroring the existing
   `apply_tests` module) confirming `select`/`confirm`/`input`/`editor` reach `on_extension_dialog`
   while `notify`/`setStatus`/unknown methods and `ExtensionError` are routed correctly.
2. Regenerate bindings (`scripts/build-rust.sh`); `swiftc -typecheck` to confirm the new Swift API
   shape (`ExtensionDialogRecord`, `ExtensionDialogReply`, `onExtensionDialog`,
   `replyExtensionDialog`) before touching Xcode/SwiftUI code.
3. `AppModel`: `pendingDialogs`/`currentDialog`, `onExtensionDialog`, `replyToCurrentDialog`;
   extend `abort()` and `onActiveSessionChanged`/`onHistoryReplaced` to clear pending dialogs per
   Design.
4. New `ExtensionDialogView.swift` (the `Identifiable` conformance + the four per-kind
   alert/sheet views); wire into `ChatView`; add the new file to `PiMac.xcodeproj/project.pbxproj`
   (same step SW4 did for `ModelsPanelView.swift`/`HfSearchView.swift`).
5. `examples/spike_check.rs`: new `dialogs` mode — send a prompt likely to trigger the installed
   `pi-permission-system` extension's gating (e.g. an `echo` bash command not covered by its
   allow-list), assert `on_extension_dialog`-equivalent data arrives with `method` `"confirm"` or
   `"select"`, reply `Cancelled`, confirm no hang (mirrors this session's own ad hoc Python probe,
   made a permanent, real, automated check).
6. Verification (below).

### Risks

- **Dialog deadlocks on abort/session-switch** (the M4 doc's own top risk for this surface): pi's
  actual behavior when a gated tool call is aborted mid-dialog — does the extension itself cancel
  the pending request, or does it sit until timeout? — is unconfirmed. Mitigated defensively (not
  proven correct) by Swift proactively replying `Cancelled` on `abort()`; flagged for real-machine
  observation during verification, not assumed safe.
- **Session-switch dialog staleness**: clearing `pendingDialogs` without a reply on
  `onActiveSessionChanged`/`onHistoryReplaced` assumes the old client (and whatever spawned the
  gated extension call) is already gone by then (`SwitchProject` drops the old `PiClient`,
  `kill_on_drop`). For same-client actions (`SwitchSession`, which doesn't respawn `client`), a
  pending dialog from the *previous* session context could in principle still be live and get
  silently dropped client-side with no reply ever sent — the extension would then hang until its
  own timeout rather than getting an explicit answer. Edge case, not exercised by the installed
  permission extension's own bash/write/edit gating (which is scoped to the current turn, not
  cross-session); revisit if seen in practice.
- **No confirmed permission-card title convention** — explicit scope cut (Design), not an
  oversight; if a real convention solidifies later (in this extension or another), dedicated
  card rendering is a small, additive follow-up, not a redesign.
- **UniFFI enum-with-associated-data support** (`ExtensionDialogReply`) — this codebase's existing
  UniFFI enums/errors (`PiSessionError`) already carry `String` payloads successfully, so this is
  low risk, but unconfirmed for a plain (non-`Error`) `uniffi::Enum` until implementation.

### Acceptance criteria

- `cargo fmt --check && cargo clippy --workspace && cargo test --workspace` green, including the
  new `dialog.rs` conversion tests and `apply()` dialog-routing tests.
- `xcodebuild -project PiMac.xcodeproj -scheme PiMac build` succeeds from a clean
  `Generated`/`PiCoreFFI.xcframework` rebuild.
- Real-machine verification against the already-installed `pi-permission-system` extension: send a
  prompt whose bash command isn't covered by its allow-list; confirm a native dialog appears in
  the Swift app showing the extension's real gating message; tapping Allow/Deny is reflected both
  in pi's behavior (command runs or is blocked) and in
  `~/.pi/agent/extensions/pi-permission-system/logs/*-permission-review.jsonl`'s resulting
  `permission_request.*` event. Separately exercise `select`/`input`/`editor` if/when a suitable
  extension is available, or via the new `spike_check.rs` `dialogs` mode for `confirm`/`select` at
  minimum. Abort a turn with a dialog pending and confirm the app doesn't hang and the dialog
  clears.

---

## Milestone SW6 — Composer model picker + server-dot health indicator in Swift

**Goal:** let the user see and change which model pi currently has selected, right from the
composer, and show a status-bar dot reflecting whether the local server backing that model is
actually healthy — the two pieces SW4 deliberately deferred (its panel is browse/manage-only)
because both depend on tracking "which model pi currently has selected," a concept the Swift app
has had no UI for until now.

### Verified facts

- **Model state is pull-based; nothing pushes a change.** `pi-rpc`'s `Event` enum
  (`crates/pi-rpc/src/types.rs:206-322`) has no model-change variant — an unrecognized future push
  would fall into `#[serde(other)] Unknown` (320-321), undetectable. `Command::GetState` (62),
  `Command::GetAvailableModels` (70), and `Command::SetModel{provider, model_id}` (65-68,
  camelCase) are the only relevant wire commands; `PiClient` wraps the first three
  (`crates/pi-rpc/src/client.rs:288-307`). `Command::CycleModel` (69) **is dead code** — defined in
  the wire enum, never sent anywhere in this codebase (confirmed by grep) — not worth adding a
  Swift entry point for a feature the Slint reference itself doesn't use.
- **`GetAvailableModels`'s response** (`{"models": [{id, name, provider, baseUrl, cost: {input,
  output}}, ...]}`) and **`GetState`'s** (includes `model: {id, provider}` at minimum, alongside
  `isStreaming`/`sessionFile`/`thinkingLevel`) are both untyped `serde_json::Value` in `pi-rpc` —
  established by how `pi-core::backend::refresh_models` (`crates/pi-core/src/backend.rs:2500-2540`)
  and `model_label` (2647-2671) consume them, not by any typed struct in `pi-rpc` itself.
- **Slint's reference logic**, all in `pi-core/src/backend.rs`, ported (not reused directly — this
  crate doesn't depend on `pi-core`, matching its established posture) into a new local module:
  - `ModelEntry{provider, id, base_url, is_local}` / `ModelsState{entries, labels, current: i32}`
    (808-837).
  - `refresh_models(client, transcript) -> ModelsState` (2500-2540): `GetAvailableModels` builds
    entries + `model_label` strings; `GetState`'s `model/id`+`model/provider` locate the matching
    entry index (or `-1`). Called after session start, and after every action that could change
    model *availability* (`ServeRapidMlxModel`, `LoadRouterModel`, `AddOllamaToPi`, `SaveApiKey`) —
    each of these does a **full re-fetch**, not a local patch.
  - `model_label(m)` (2645-2671): `"{name} · {provider}"`, plus `" · free · local"` when
    `is_local_base_url(baseUrl)` (2627-2643: host is `localhost`/`127.0.0.1`/`0.0.0.0`/`[::1]`/
    `::1`), else `" · ${input}/${output}"` when `cost.input`/`cost.output` are present.
  - `UiCmd::SetModel(usize)` (1256-1268): `client.set_model(provider, id)`, then — uniquely for
    this one action — patches `models.current` **in place** (not a re-fetch) since the index is
    already known, refreshes thinking levels (differ per model — see Design's scope cut), and
    calls `dot_interval.reset_immediately()`.
- **Server dot**, also all in `pi-core/src/backend.rs`, untouched by any prior SW extraction:
  - Constants `SERVER_DOT_HIDDEN/OK/DOWN/MISMATCH` (847-854, `0..3`).
  - `compute_server_dot(current: Option<&ModelEntry>, managed: &mut Option<ManagedRapidMlx>) ->
    i32` (2542-2569): hidden unless `current.is_local`; for provider `"rapid-mlx"`, a real
    `GET /health` via `pi_local::rapid_mlx::server_health` (already reachable — `pi-core-ffi`
    depends on `pi-local` since SW4) classified by `classify_rapid_mlx_dot` (2571-2598, a pure,
    4-branch truth table: healthy+serving-current → OK, serving-current-but-not-ready → DOWN,
    healthy-but-serving-a-different-model → **MISMATCH** (a rapid-mlx server 404s completions for
    any model other than the one it's serving — a plain reachability check would lie), unreachable
    → `managed_alive` breaks the tie between DOWN and OK for this app's own managed child); any
    other local provider gets a generic 1s-timeout TCP connect, `probe_tcp` (2604-2623), no HTTP
    semantics.
  - A `tokio::time::interval(Duration::from_secs(5))` (`MissedTickBehavior::Delay`) inside
    `run_session`'s existing `tokio::select!`, pushing `ui.set_server_dot` **only on change**;
    `dot_interval.reset_immediately()` is called after `SetModel`, `ServeRapidMlxModel`, and
    `ServerDotClicked`'s restart branch, so a just-made change doesn't wait up to 5s.
  - `UiCmd::ServerDotClicked`: restarts a dead managed rapid-mlx server, else opens the models
    panel — an explicit scope cut here (see Design).
- **`pi-core-ffi`/Swift currently have none of this.** `PiSession`'s exported surface (`lib.rs:
  175-354`) has no `get_state`/`get_available_models`/`set_model`; `ChatSink` has no model/dot
  callback; `LocalModelIndex` is structurally unable to answer "what does pi currently have
  selected" — it's stateless, no live `PiClient` (`local_models.rs:122-124`), and its
  `RouterModelRecord.loaded`/`busy` describe the *router server's* load state, orthogonal to
  "is pi pointed at it." Both `local_models.rs:128-130` and `ModelsPanelView.swift:7` already carry
  doc comments naming this exact deferral. New `PiSession`-based RPC surface is required, not reuse
  of existing SW4 records.
- **Swift attach points**: `ChatView.swift`'s composer `HStack` (41-60, `TextField` + Send/Abort)
  for the picker; `statusBar`'s existing `Circle()` streaming dot (68-71) is the exact pattern to
  mirror for the server dot.

### Design

- **New `pi-core-ffi/src/models.rs`**: `ModelRecord{provider: String, id: String, label: String,
  is_current: bool}` (UniFFI `Record`) and `ServerDotState` (UniFFI `Enum`: `Hidden | Ok | Down |
  Mismatch` — typed, not a raw `i32`, for a nicer Swift `switch`). Private `ModelEntry` (mirrors
  pi-core's), plus ported-not-shared pure functions `model_label`/`is_local_base_url`/`probe_tcp`/
  `classify_rapid_mlx_dot`/`compute_server_dot` (async) — unit-tested against the same cases
  pi-core's own `server_dot_tests` module covers (the 4-branch truth table, including
  `serving_a_different_model_is_a_mismatch_not_ok`). One shared helper,
  `refresh_models_and_state(client) -> (Vec<ModelRecord>, Option<ModelEntry>)`, ports
  `refresh_models` — called uniformly from every action that touches model state (see next bullet),
  rather than also porting `SetModel`'s local-patch shortcut: one code path, no duplicated
  "locate by index" logic, and it already matches Slint's own `ServeRapidMlxModel` precedent (a
  full re-fetch, not a patch).
- **`run()` gains `current_model: Option<ModelEntry>` and `last_dot: ServerDotState`** (loop-scoped
  mutable locals, same pattern as `client`/`events`/`managed_rapid_mlx`), plus a `dot_interval`
  arm in the existing `tokio::select!`, calling `compute_server_dot(current_model.as_ref(), &mut
  managed_rapid_mlx)` and pushing `sink.on_server_dot_changed(dot)` only on change — a direct port
  of `run_session`'s timer. New `ChatCmd::RefreshModels{reply}` and `ChatCmd::SetModel{provider,
  model_id, reply}` (oneshot-reply, same shape as `switch_project`/etc.) both call
  `refresh_models_and_state` and update `current_model`; `SetModel`'s handler calls
  `client.set_model(...)` first, then the same refresh, then `dot_interval.reset_immediately()`.
  `ServeRapidMlxModel`'s existing handler (SW4) gets the same two additions: refresh
  `current_model` afterward and reset the dot timer — it already calls `client.set_model(...)`
  internally, so this closes the one place server-dot state could otherwise go stale.
- **`ChatSink` gains one method**: `fn on_server_dot_changed(&self, state: ServerDotState)`. The
  model *list*/*current index* stays pull-based (`PiSession::refresh_models`/`set_model`, both
  async `Result`-returning via the established `self.call(...)` helper) — matching how
  `SessionIndex`/`LocalModelIndex` already work, not a new push pattern; only the dot (a
  5-second-polled value with no natural "user action" to hang a re-fetch off of) needs a push.
- **Explicit scope cuts**, flagged so none read as an oversight: no click-to-restart on the dot
  (Slint's `ServerDotClicked` — the Models panel's existing "Serve" button already reaches the same
  action, just one click further away); `CycleModel` stays unimplemented (dead in the reference
  app too); thinking-level control (a related but separate composer element in Slint, refreshed
  alongside the model picker since available levels differ per model, but not named in this
  milestone's scope) is a natural, similarly-shaped follow-up, not silently included here.
- **SwiftUI**: `AppModel` gains `private(set) var models: [ModelRecord] = []`, `private(set) var
  serverDot: ServerDotState = .hidden`, `refreshModels()` (pull, mirrors `refreshModelsPanel`),
  `setModel(provider:modelId:)` (calls `session.setModel`, then `refreshModels()` —
  action-then-refetch, this codebase's established pattern), and `onServerDotChanged` `ChatSink`
  conformance. `refreshModels()` is called once in `start()` (alongside the existing
  `refreshSidebar()` call) and added to `serveRapidMlx`/`loadRouterModel`/`downloadHfModel`'s
  existing post-action refresh points (small additions to SW4 methods — keeps the composer
  picker's availability list in sync with router/rapid-mlx actions the same way Slint's
  `refresh_models` re-runs after those `UiCmd`s). New composer `Menu` in `ChatView`'s `HStack`
  listing `model.models` by `label`, checkmarking `isCurrent`, each row calling
  `model.setModel(provider:modelId:)` — a `Menu` (not `Picker`) since per-row action closures need
  no separate `@State` selection binding kept in sync with `isCurrent`, matching this codebase's
  "plainest SwiftUI primitive that fits" pattern (e.g. `ExtensionDialogView`'s plain `List`/
  `Button`s over a custom component). Server dot: a `Circle()` in `statusBar` beside the existing
  streaming dot, styled by `model.serverDot` (`.hidden` → not rendered, `.ok` → green, `.down` →
  red, `.mismatch` → amber) — the exact color semantics `app.slint`'s dot already uses.

### Work breakdown

1. `pi-core-ffi`: new `src/models.rs` — `ModelRecord`, `ServerDotState`, private `ModelEntry`,
   `model_label`/`is_local_base_url` (unit-tested: free/local, priced, no-price cases),
   `probe_tcp`, `classify_rapid_mlx_dot` (unit-tested: the same 4-branch truth table as pi-core's
   `server_dot_tests`), `compute_server_dot` (async), `refresh_models_and_state`.
2. `ChatCmd::RefreshModels`/`SetModel`; `run()`'s `current_model`/`last_dot`/`dot_interval` state +
   new `tokio::select!` arm + two new command-handler arms; `ServeRapidMlxModel`'s handler updated
   to also refresh `current_model` and reset the dot timer; `ChatSink::on_server_dot_changed`;
   `PiSession::refresh_models`/`set_model`. `run_demo`/`reply_demo_action` gets matching arms (a
   couple of synthetic `ModelRecord`s + `ServerDotState::Hidden`) so demo mode isn't broken.
3. Regenerate bindings (`scripts/build-rust.sh`); `swiftc -typecheck` to confirm the new Swift API
   shape before touching Xcode/SwiftUI code.
4. `AppModel`: `models`/`serverDot` published state, `refreshModels()`, `setModel(provider:
   modelId:)`, `onServerDotChanged`; wire `refreshModels()` into `start()` and into
   `serveRapidMlx`/`loadRouterModel`/`downloadHfModel`.
5. `ChatView`: composer `Menu` for the model picker; extend `statusBar` with the server-dot
   `Circle()`.
6. `examples/spike_check.rs`: extend with a mode calling `refresh_models`/`set_model` against a
   real `pi`, printing the resulting labels/current index — verifiable without rapid-mlx/router
   installed, since it exercises whatever models `pi`'s own config already lists.
7. Verification (below).

### Risks

- **`GetAvailableModels`/`GetState` field shapes are asserted, not typed** (both are raw `Value` in
  `pi-rpc`) — same posture pi-core already accepts; a future pi wire-format change would degrade
  gracefully (empty label list / no current match) rather than crash.
- **Two near-identical `ModelEntry`/`refresh_models` implementations** (pi-core's, this crate's)
  will drift if pi's wire shape changes — accepted, matches this crate's established
  no-pi-core-dependency posture used throughout SW1-SW5.
- **No click-to-restart on the dot** — explicit scope cut (Design), not an oversight.
- **`Interval::reset_immediately()`** is already used successfully in `pi-core` against the same
  workspace-pinned `tokio = "1"`, so low risk, but unconfirmed in this specific crate until
  implementation.

### Acceptance criteria

- `cargo fmt --check && cargo clippy --workspace && cargo test --workspace` green, including new
  `models.rs` unit tests (label formatting, `classify_rapid_mlx_dot`'s 4-branch truth table).
- `xcodebuild -project PiMac.xcodeproj -scheme PiMac build` succeeds from a clean rebuild.
- Real `pi --mode rpc` verification: the composer picker lists pi's actual configured models with
  correct labels (local/free vs priced), selecting one calls through to `pi` and is reflected as
  the new checkmarked entry on next refresh; wherever rapid-mlx is actually running locally, the
  server dot shows green/red/amber correctly (cross-checked against a manual `curl <base>/health`)
  and updates within ~5s (or immediately after switching models) — otherwise the dot stays hidden
  for cloud models.

---

## Milestone SW7 — Live thinking/tool-call visibility in Swift

**Goal:** show "thinking" (reasoning) content and tool-call execution live, while a turn is still
streaming, instead of the Swift app staying completely blind to them until the turn settles. This
was flagged (not silently dropped) as an explicit scope cut in both the SW3 milestone and the
live-streaming-rendering fix — this closes it.

### Verified facts

- **Thinking events are nested, not top-level** — `AssistantMessageEvent::ThinkingStart/
  ThinkingDelta{delta}/ThinkingEnd` (`crates/pi-rpc/src/types.rs:135-179`) arrive inside
  `Event::MessageUpdate{assistant_message_event: ...}`, the same envelope `TextDelta` already
  uses. `ThinkingDelta` carries **no id/index field** — pi assumes at most one thinking block live
  at a time (positional, not keyed). `AssistantMessageEvent` also declares `ToolcallStart/Delta/
  End` variants (162-168) that are **dead code** — never matched by `pi-core`'s own
  `apply_delta`, confirming tool-call rendering is driven entirely by the mechanism below instead.
- **Tool-call events are top-level, keyed by `tool_call_id: String`** —
  `Event::ToolExecutionStart{tool_call_id, tool_name, args}` / `ToolExecutionUpdate{tool_call_id,
  tool_name, args, partial_result}` / `ToolExecutionEnd{tool_call_id, tool_name, result,
  is_error}` (`types.rs:238-264`). This id is the join key both the live path (below) and the
  already-working finalized path (`pi_render::hydrate_rowspecs`'s `tool_rows: HashMap<String,
  usize>`) use to patch the right row.
- **`pi-core`'s live reference** (`crates/pi-core/src/backend.rs`) uses two throttle constants,
  already documented in this project's CLAUDE.md: `TEXT_FLUSH = 33ms` (reused for thinking, not
  just text) and `TOOL_FLUSH = 100ms` (tool-call partial-output updates only) (42-43). State:
  `thinking: Option<ThinkingRegion>{row, buffer, last_flush, first}` (217-222, at most one live
  region — a fresh `ThinkingStart` mid-turn replaces it with a new row, it doesn't reuse the old
  one) and `tools: HashMap<String, ToolRun>{row, name, summary, args_pretty, started, last_flush}`
  (224-231, supports concurrent tool calls, each its own row). Unlike text's `StreamRegion`/
  `flush_stream` (which re-segments and diffs a growing markdown buffer into a variable number of
  rows), thinking/tool state is simpler: each is exactly **one fixed row**, addressed in place via
  `self.ui.set(row, spec)` — no segmentation, no diffing.
  - `ThinkingStart` → `push_row` a new `RowSpec{kind:"thinking", running:true}`, store the row
    index. `ThinkingDelta` → append to `buffer`, flush to `ui.set` only if `last_flush.elapsed() >=
    TEXT_FLUSH` (436-466). `ThinkingEnd` → force one final `ui.set` with `running:false`,
    regardless of the gate (467, 480-491) — also force-finalized defensively on `AgentSettled`/
    `MessageEnd`.
  - `tool_start` (493-516): `push_row` a new `RowSpec{kind:"tool", text:"⚙ {tool_summary(name,
    args)}", detail: pretty-printed args, running:true}`, insert into `tools` keyed by
    `tool_call_id`. `tool_update` (518-535): gated by `TOOL_FLUSH`, patches `detail` with
    `"{args_pretty}\n───\n{tail(content_text(partial_result), TOOL_DETAIL_LIMIT)}"`. `tool_end`
    (537-557): **removes** the map entry, does one final unconditional `ui.set` with a `✓`/`✗`
    mark, the full result tail, and `elapsed: format_elapsed(started.elapsed())` (793-802, exact
    source verified: `"{secs:.1}s"` under 10s, `"{secs:.0}s"` under 60s, else `"{}m {:02}s"`).
- **The finalized path (already working over FFI since SW3) builds field-for-field-identical
  `RowSpec`s** via `pi_render::hydrate_rowspecs` — confirms `RowRecord` needs **no new fields**:
  `kind`, `text`, `detail`, `running`, `elapsed` are exactly what both the live and finalized paths
  already populate. The one difference: hydration never sets `elapsed` on tool rows (no wall-clock
  start time available from `get_messages` history) — only the live path can, which is one more
  reason live visibility is worth having, not just a formatting nicety.
- **`tool_summary`/`tail`/`TOOL_DETAIL_LIMIT` are already `pub` at `pi_render`'s crate root**
  (`crates/pi-render/src/lib.rs:14-17`) and **`content_text` at `pi_rpc`'s** (`pi-rpc/src/
  types.rs:334`, re-exported at `pi_rpc`'s root) — `pi-core-ffi` already depends on both crates, so
  reusing this exact formatting logic (not reimplementing it) needs no new dependency.
  `format_elapsed` itself has no `pi_render`/`pi_rpc` home (it's `pi-core`-only, `backend.rs:793`)
  — small enough to reimplement locally, matching this crate's established posture (e.g.
  `models.rs`'s `probe_tcp`/`classify_rapid_mlx_dot`).
- **`pi-core-ffi`'s `apply()` (`crates/pi-core-ffi/src/lib.rs`, ~935-966) drops all of this today**
  — its match arms cover `AgentStart`/`AgentSettled`/`TextDelta`/`MessageEnd`/`Error`/the SW5
  dialog arms only; `ThinkingStart/Delta/End` (nested, never matched) and `Event::
  ToolExecutionStart/Update/End` (top-level, unmatched) both fall into the final `_ => {}`.
  `ChatSink` has no thinking/tool callback. `RowRecord` (`row.rs`) already mirrors every
  `RowSpec` field needed — confirmed no changes needed there.
- **`PiSession::preview_rows`** (SW3, `lib.rs` ~404-414 delegating to a free fn ~718-724) is the
  established FFI precedent for live content — but it's deliberately **stateless and Swift-pulled**:
  Swift accumulates raw text itself (`onTextDelta` appends to a buffer) and calls `previewRows`
  on its own ~33ms throttle, because turning one markdown buffer into rows is a pure function of
  that buffer alone. This precedent **does not fit tool calls**: they're keyed, concurrent, and
  need `tool_summary`/`tail` formatting that would mean duplicating real Rust logic in Swift to
  pull-model it. See Design for why this milestone uses a different (push-based) shape instead.
- **Swift-side rendering needs zero new view code.** `RowView.swift`'s `ThinkingRowView` (reads
  `row.running`/`row.text` only) and `ToolRowView` (reads `row.running`/`row.text`/`row.elapsed`/
  `row.detail`, already has its own expand/collapse `@State`) already render exactly this shape
  correctly, dispatched from `RowView`'s existing `switch row.kind` — they were built against
  `hydrate_rowspecs`' finalized output and need no changes to also render live-in-progress rows.
  `AppModel`'s existing `streamingRows`/`transcript` clear-on-`onTurnEnd`/`onHistoryReplaced`
  pattern is the direct precedent for this milestone's own live-state lifecycle.

### Design

- **Push-based, not pull-based — a deliberate departure from `preview_rows`' shape, not an
  inconsistency.** Tool-call rows need `tool_summary`/`tail` formatting that only makes sense
  running once, server-side, against real state Rust already tracks — pulling would mean Swift
  either re-deriving that formatting itself (duplicating real logic) or receiving the raw
  name/args/partial-result and computing nothing itself (at which point pulling buys nothing over
  pushing). Since tool calls must be push-based, thinking is push-based too for one consistent
  story rather than a hybrid — even though thinking's single-buffer shape would *tolerate* a pull
  design the way text does.
- **New `pi-core-ffi/src/live_preview.rs`**: `ThinkingRegionState{id: String, buffer: String,
  last_flush: Instant}`, `ToolRunState{summary: String, args_pretty: String, started: Instant,
  last_flush: Instant}` (both internal, not UniFFI types), a ported `format_elapsed`, and small
  `RowRecord`-building helpers mirroring `tool_start`/`tool_update`/`tool_end`'s exact `RowSpec`
  shape. The core function: `fn apply_live_preview(event: &Event, sink: &dyn ChatSink, thinking:
  &mut Option<ThinkingRegionState>, tools: &mut HashMap<String, ToolRunState>, thinking_seq: &mut
  u64)` — a **new** function, not a change to `apply()`'s existing signature, so `apply()` and its
  existing `apply_tests` stay untouched; `run()` calls both `apply(&event, ...)` and `
  apply_live_preview(&event, ...)` for every received event.
- **`ChatSink` gains two keyed callbacks**:
  ```rust
  /// Once (running:true, empty text) on ThinkingStart, throttled ~33ms
  /// (TEXT_FLUSH, reused for thinking) on ThinkingDelta, once more
  /// (running:false) on ThinkingEnd. `id` is a synthetic per-region
  /// sequence number — thinking has no id of its own on the wire, and pi
  /// allows more than one thinking block per turn (each becomes its own
  /// row, never reusing a prior region's).
  fn on_thinking_row_changed(&self, id: String, row: RowRecord);
  /// Once (running:true) on ToolExecutionStart, throttled ~100ms
  /// (TOOL_FLUSH) on ToolExecutionUpdate, once more (running:false, ✓/✗
  /// mark, elapsed) on ToolExecutionEnd. `id` is the real `tool_call_id`
  /// — multiple calls can be live at once.
  fn on_tool_row_changed(&self, id: String, row: RowRecord);
  ```
  No "removed"/`None` signal needed for either: a finished row simply stops updating and stays at
  its final state until the next `AgentSettled`'s `hydrate_and_push` → `on_history_replaced` wipes
  and replaces *all* live-preview state at once — the same clear-on-settle contract
  `streamingRows`/`transcript` already have, extended to cover these two new arrays too.
  `run()`'s existing `AgentSettled` branch also resets `live_thinking`/`live_tools`'s Rust-side
  state (defensive — guards a tool call that never received its `ToolExecutionEnd`).
- **No new `ChatCmd` variant** — this is purely event-driven from `events.recv()`, nothing Swift
  ever calls to trigger it. `run_demo` needs no changes (it never forwards real `pi_rpc::Event`s at
  all); `RecordingSink`/`examples/spike_check.rs`'s `PrintSink` need the two new `ChatSink` methods
  implemented to keep compiling, same as every prior `ChatSink` addition.
- **Explicit scope cut: no exact interleaved chronological ordering** against the eventual
  finalized transcript. Swift renders `model.rows`, then all live thinking blocks (append/upsert
  order), then all live tool rows (append/upsert order), then `model.streamingRows` — a stable,
  simple ordering, not a merged single timeline across two independently-keyed live channels.
  Acceptable because this ordering is only ever visible transiently while a turn streams; the
  authoritative, correctly-ordered `onHistoryReplaced` rows replace it the instant the turn
  settles. Flagged so it isn't mistaken for an oversight later.
- **Swift**: `AppModel` gains a small `LiveRow: Identifiable { let id: String; var row: RowRecord
  }` wrapper and `private(set) var liveThinkingRows: [LiveRow] = []` / `liveToolRows: [LiveRow]
  = []`, each upserted-by-`id` in `onThinkingRowChanged`/`onToolRowChanged`. Both arrays clear
  alongside `streamingRows`/`transcript` in `onTurnEnd` and `onHistoryReplaced`. `ChatView` gets
  two more `ForEach`s (using `RowView`, no new view code) inserted between the `model.rows` and
  `model.streamingRows` loops, and their `.count`s join the existing auto-scroll `.onChange`
  triggers.

### Work breakdown

1. `pi-core-ffi`: new `src/live_preview.rs` — `ThinkingRegionState`/`ToolRunState`, ported
   `format_elapsed`, `RowRecord`-building helpers, `apply_live_preview`. `ChatSink::
   on_thinking_row_changed`/`on_tool_row_changed`; `run()` gains `live_thinking`/`live_tools`/
   `thinking_seq` loop-scoped state, the new call alongside `apply()`, and the `AgentSettled`
   branch's defensive reset. `RecordingSink`/`spike_check.rs`'s `PrintSink` updated. Unit tests:
   `format_elapsed`'s three branches; a thinking start/delta/end sequence via `RecordingSink`
   asserting the running/text progression and the 33ms throttle gate; a tool start/update/end
   sequence asserting the ✓/✗ mark, elapsed, and detail formatting; two concurrent tool calls
   (distinct `tool_call_id`s) not clobbering each other.
2. Regenerate bindings (`scripts/build-rust.sh`); `swiftc -typecheck` to confirm the new Swift API
   shape (`onThinkingRowChanged`/`onToolRowChanged`) before touching Xcode/SwiftUI code.
3. `AppModel`: `LiveRow`, `liveThinkingRows`/`liveToolRows`, the two new `ChatSink` methods
   (upsert-by-id), clearing them in `onTurnEnd`/`onHistoryReplaced`.
4. `ChatView`: the two new `ForEach`s between `model.rows` and `model.streamingRows`; extend the
   auto-scroll `.onChange` triggers.
5. Verification (below).

### Risks

- **No exact interleaved ordering vs. the finalized transcript** — explicit scope cut (Design), not
  an oversight; revisit with a genuine merged-timeline design only if this proves confusing in
  real use, not preemptively.
- **Orphaned tool calls** (a `ToolExecutionEnd` that never arrives, e.g. on an unusual abort path)
  — mitigated by clearing Rust-side `live_tools`/`live_thinking` defensively on `AgentSettled`, but
  not specifically on `abort()` (pi-core's own reference has no explicit handling here either, per
  research); flagged as a minor edge case, not proactively solved without concrete evidence it
  matters in practice.
- **Model/extension dependent**: whether a live thinking row ever appears at all depends on the
  configured model actually emitting `ThinkingDelta`s (not all models/thinking levels do) — real
  verification should note this rather than treat an absent thinking row as a bug if the active
  model doesn't reason visibly.

### Acceptance criteria

- `cargo fmt --check && cargo clippy --workspace && cargo test --workspace` green, including the
  new `live_preview.rs` unit tests.
- `xcodebuild -project PiMac.xcodeproj -scheme PiMac build` succeeds from a clean rebuild.
- Real `pi --mode rpc` verification: send a prompt that triggers a real tool call (e.g. "read
  Cargo.toml and summarize it") and confirm a tool row appears and updates *while the turn is still
  streaming* (spinner via the existing `ProgressView`, expandable detail via the existing
  disclosure `@State`), not only after `onHistoryReplaced` fires; if the active model/thinking
  level actually emits reasoning content, confirm a live thinking row similarly appears and grows
  during streaming; confirm the transition to the finalized rows at turn-end is clean (no
  duplicated or stuck live rows left behind).

---

## Fix — Native NSPasteboard copy in Swift

**Goal:** match the Slint app's existing per-code-block and per-message copy affordance in Swift,
using `NSPasteboard` in place of `arboard`. Pure UI work — no `pi-core-ffi`/RPC changes, since
`RowRecord` already carries every field needed (confirmed below).

### Verified facts

- **Slint's only clipboard call site** is UI-glue in `crates/slinty-pi/src/main.rs:103-107`:
  `app.on_copy_text(move |text| { if let Ok(mut clipboard) = arboard::Clipboard::new() {
  let _ = clipboard.set_text(text.to_string()); } })` — content-agnostic, just writes whatever
  string the UI handed it. `arboard` is a dependency of `crates/slinty-pi` only, never `pi-core`;
  there is no `UiCmd::Copy` variant and no copy-related code anywhere in `pi-core`/`pi-render`
  beyond the data fields themselves (confirmed by grep). Trigger is **click-only** — no keyboard
  shortcut, no command-palette entry.
- **The UI affordance** (`crates/slinty-pi/ui/app.slint`) is a small reusable `CopyButton`
  component (114-143): a click-only chip showing `"copy"` that flips to `"✓"` for 1.4s
  (`Timer { interval: 1400ms; ... }`) after being clicked. It's embedded in exactly four row
  components:
  - `CodeRow` (322-339): **always** shown, in the row's header bar next to the language label;
    `payload: entry.text` — the fence-stripped code content.
  - `ProseRow`/`HeadingRow`/`QuoteRow` (236-253, 255-274, 276-299): shown **only when
    `entry.first && entry.raw != ""`** — once per message group, not once per segmented row;
    `payload: entry.raw` — the full raw markdown of the *entire enclosing message*, not just this
    one segment.
  - `table`/`thinking`/`tool`/`user`/`info`/`error`/`rule` rows get **no** `CopyButton` anywhere in
    `app.slint` — confirmed by direct inspection of each component.
- **`pi_render::RowSpec::raw`'s doc comment spells out the design intent verbatim**
  (`crates/pi-render/src/hydrate.rs:37-42`): "The full original markdown of this row's enclosing
  message/text block, shared by every row segmented out of it — used for the per-message copy
  affordance, which copies the source text rather than any one rendered/segmented piece of it.
  Empty where a group copy isn't offered (thinking/tool/info rows)." — confirms this field exists
  *specifically* for this feature and was already threaded through to `RowRecord` unchanged since
  SW3, just never consumed by anything in Swift.
- **`RowRecord`** (`crates/pi-core-ffi/src/row.rs:60-74`) — confirmed current, unchanged:
  `kind, markdown, text, lang, level, detail, running, elapsed, first, raw, code_lines,
  table_rows`. The exact mapping to replicate: `kind == "code"` → copy `row.text`; `kind ==
  "prose" | "heading" | "quote"` → copy `row.raw`, gated on `row.first && !row.raw.isEmpty`; every
  other kind → no button (matches Slint exactly, including the `table` gap — `raw` is populated
  for table rows but never exposed via a button in `app.slint` either).
- **Swift currently has zero clipboard code** (`grep -ri "pasteboard\|clipboard" apps/pi-mac` →
  no matches anywhere, including generated bindings). `RowView.swift`'s `CodeBlockView` (116-161)
  is fully static — no hover state, no header interactivity to extend. `ToolRowView` (83-114) has
  the app's only existing per-row interactive chrome (`@State private var expanded`, a plain
  `Button`) — a directly reusable *pattern* (local `@State` + plain `Button`) even though `tool`
  rows themselves get no copy button. `prose`/`heading`/`quote` render as bare `Text` in `RowView`
  directly, no wrapping container to hang conditional chrome off of yet.

### Design

- **New private `CopyButton` view in `RowView.swift`**, mirroring `app.slint`'s component exactly:
  `@State private var copied = false`; on tap, `NSPasteboard.general.clearContents()` +
  `.setString(payload, forType: .string)`, flip `copied = true`, then a `Task { try? await
  Task.sleep(for: .milliseconds(1400)); copied = false }` — the direct Swift analog of the
  `.slint` component's `Timer`. Needs `import AppKit` (new import for this file — `NSPasteboard`
  isn't in SwiftUI/Foundation).
- **`CodeBlockView`**: wrap the existing conditional `Text(row.lang)` in a header `HStack` with a
  `Spacer()` and an unconditional `CopyButton(payload: row.text)` — matches `CodeRow`'s "always
  shown" behavior exactly. The language label itself keeps its current show-only-if-non-empty
  behavior unchanged (that's an unrelated, pre-existing display choice, not part of this fix).
- **`RowView`'s `"prose"`/`"quote"`/`"heading"` cases**: a small `withGroupCopy` helper wraps the
  existing content in an `HStack` with a trailing `CopyButton(payload: row.raw)` when `row.first &&
  !row.raw.isEmpty`, else returns the content unchanged — the direct Swift mirror of Slint's
  `if entry.first && entry.raw != ""` gate.
- **Every other row kind is untouched** — no button added to `table`/`thinking`/`tool`/`user`/
  `info`/`error`/`rule`, matching Slint's own scope exactly rather than expanding it. If a
  table/tool copy affordance is ever wanted, that's a new, separate, deliberate scope decision
  (Slint doesn't have it either today), not something this fix should quietly add.

### Work breakdown

1. `RowView.swift`: `import AppKit`; new private `CopyButton` view; `CodeBlockView`'s header
   `HStack` + unconditional `CopyButton`; `RowView`'s `withGroupCopy` helper wrapping `prose`/
   `quote`/`heading`'s existing content, gated on `row.first && !row.raw.isEmpty`.
2. Verification (below).

### Verification

- `swiftc -typecheck` against the current generated bindings (no bindings regeneration needed —
  zero `pi-core-ffi` changes); clean `xcodebuild -project PiMac.xcodeproj -scheme PiMac build`.
- Manual: send a prompt producing a code block, click its copy chip, confirm the system clipboard
  now holds the fence-free code (paste into a text field to check) and the chip flips to "✓" for
  ~1.4s; confirm a prose message's copy chip (shown once, on the first row of that message) copies
  the full raw markdown source, not just the one visible segment; confirm no copy chip appears on
  tool/thinking/table/user/info/error rows, matching Slint.

---

## Fix — User message bubble styling + delete-session confirmation

**Goal:** two small, unrelated polish items requested together — make the user's own prompts
visually distinct in the transcript, and stop session deletion from being a single accidental
click away. Both pure Swift, zero `pi-core-ffi`/RPC changes.

### Verified facts

- **Slint's `UserRow`** (`crates/slinty-pi/ui/app.slint:214-234`): right-aligned via a leading
  stretch-spacer `Rectangle` pushing the bubble to the right, `background: Palette.accent-
  background.transparentize(0.88)` (an 88%-transparent accent tint), `border-radius: Style.radius`,
  11px padding, 14px text. Swift's current `RowView`'s `"user"` case (`RowView.swift:42-45`) is
  plain left-aligned `Text(row.text).fontWeight(.medium)` — no alignment, no background.
- **Slint's delete confirmation** is an inline double-tap-with-timeout chip, not a modal —
  `DeleteButton` (`crates/slinty-pi/ui/sidebar.slint:11-45`): first click widens "×" into a red
  "sure?" label; a second click within 2.5s fires `delete-requested()`; an unattended `Timer`
  resets it back to "×" after 2.5s. The identical `confirming` pattern is reused for "fork" in
  `tree.slint`, confirming it's a project-wide micro-interaction convention, not one-off.
- **Deletion is trash-based on both sides of the FFI boundary already** — `trash::delete` (OS
  Trash, recoverable, not permanent) in `pi-core/src/backend.rs:1469` (Slint) and `pi-core-ffi/
  src/lib.rs:562` (the exact path `AppModel.deleteSession` already calls via `PiSession.
  deleteSession`). `SidebarView.swift`'s current context-menu "Delete" button fires immediately,
  with *zero* confirmation of any kind — one step lighter than even Slint's own inline chip.

### Design

- **User bubble**: wrap `RowView`'s `"user"` case in a trailing-aligned `HStack` (`Spacer()` then
  the bubble) with a `.accentColor.opacity(0.12)`-tinted `RoundedRectangle` background —
  approximates Slint's 88%-transparent accent tint using SwiftUI's native color-opacity idiom.
- **Delete confirmation**: a native SwiftUI `.alert`/`.confirmationDialog` on the existing
  context-menu "Delete" button, mentioning Trash-recoverability in the message — a deliberate,
  idiomatic-per-platform choice over porting Slint's inline double-tap chip, consistent with this
  branch's established pattern of native equivalents over literal ports (e.g. SW5's alert/sheet
  choices, SW6's `Menu` over `ComboBox`).

### Work breakdown

1. `RowView.swift`: right-align + tinted bubble background for the `"user"` case.
2. `SidebarView.swift`: wrap the delete action in a confirmation `.alert`, noting it's
   recoverable via Trash.
3. Verification.

### Acceptance criteria

- Clean `xcodebuild`. Manual: user messages render right-aligned with a visibly tinted background,
  distinct from assistant prose; clicking "Delete" on a session prompts for confirmation before
  anything disappears from the sidebar.

---

## Fix — Transcript density control (Verbose/Normal/Summary)

**Goal:** add the Slint app's density toggle — a single control that hides thinking/tool/info rows
entirely (Summary) or force-expands every tool call (Verbose), persisted across restarts. Pure
Swift, zero `pi-core-ffi`/RPC changes — Slint's own version has no server round-trip either.

### Verified facts

- **`crates/pi-core/src/density.rs`** (71 lines): persists a plain `i32` (0/1/2) to `~/Library/
  Application Support/dev.slinty-pi.slinty-pi/state.json` as `{"density": N}`, default `1`
  (Normal). Purely client-side — no RPC, no `UiCmd` round-trip at all; `main.rs` wires `app.
  on_density_changed(density::save)` directly.
- **Naming/semantics, from `app.slint`'s own comments**: `0` = Verbose, `1` = Normal, `2` =
  Summary. `Style.row-visible(density, kind)` (`app.slint:68-79`): errors always show; density `2`
  (Summary) shows only `user`/`prose`/`heading`/`code`/`quote`/`rule`/`table` (hides `thinking`/
  `tool`/`info`); densities `0`/`1` show every kind. Applied per-row in the transcript `ListView`
  (`app.slint:743`).
- **Verbose vs. Normal is about tool-call expansion, not row filtering**: `ToolRow { force-
  expanded: root.density == 0 }` (`app.slint:782`) — Verbose forces every tool row fully expanded
  regardless of its own toggle state; Normal leaves tool rows collapsed by default (still
  individually clickable); Summary never shows tool rows at all.
- **Trigger**: a toolbar `StripChip` whose label is the current level's name (`app.slint:969-972`)
  plus **Ctrl+O** (`app.slint:729-732`), both calling `cycle-density()` (`app.slint:646-648`),
  which cycles `0 → 1 → 2 → 0 → …`.

### Design (re-verified against the current Swift sources)

- **`AppModel`** (`AppModel.swift`) gains `private(set) var density: Int`, seeded in `init()` from
  a new `UserDefaults` key — same shape as the existing `lastProjectDefaultsKey`/
  `lastSessionsDefaultsKey` pair (`AppModel.swift:400-424`), not a JSON state file:
  ```swift
  private static let densityDefaultsKey = "dev.slinty-pi.swifty-pi.density"
  private static func loadDensity() -> Int {
      (UserDefaults.standard.object(forKey: densityDefaultsKey) as? Int) ?? 1 // Normal
  }
  private static func saveDensity(_ value: Int) {
      UserDefaults.standard.set(value, forKey: densityDefaultsKey)
  }
  func setDensity(_ value: Int) {
      density = value
      Self.saveDensity(density)
  }
  ```
  **(Built as `setDensity(_:)` rather than a `cycleDensity()`, per a user follow-up request —
  see the toolbar control note below.)**
- **Pure filter**, `static func rowVisible(density: Int, kind: String) -> Bool` on `AppModel` —
  matches the existing `private static func upsert(...)` (`AppModel.swift:512-518`) as the
  precedent for a stateless static helper living on this type:
  ```swift
  static func rowVisible(density: Int, kind: String) -> Bool {
      if kind == "error" { return true }
      guard density == 2 else { return true } // Verbose/Normal: everything shows
      return !["thinking", "tool", "info"].contains(kind)
  }
  ```
- **`ChatView.swift`**: wrap each of the four existing `ForEach` bodies (`model.rows` line 19-21,
  `liveThinkingRows` 22-24, `liveToolRows` 25-27, `streamingRows` 28-30) in an
  `if AppModel.rowVisible(density: model.density, kind: row.kind) { RowView(...) }` —
  minimal-diff, no restructuring of the existing scroll/auto-scroll logic.
  **Toolbar control — built as a `Picker`, not the cycle button originally sketched here**: after
  the initial cycle-button implementation shipped, the user asked for direct 3-way selection (a
  "combo box") with a flat, non-glass appearance instead. Final shape, in the same
  `.toolbar { }` block (`ChatView.swift:90-124`), reusing the exact
  `#available(macOS 26.0, *) { ... }.sharedBackgroundVisibility(.hidden)` / plain-`ToolbarItem`
  fallback pattern the status dots already use:
  ```swift
  private var densityPicker: some View {
      Picker("Density", selection: densityBinding) {
          Text("Verbose").tag(0)
          Text("Normal").tag(1)
          Text("Summary").tag(2)
      }
      .pickerStyle(.menu)
      .labelsHidden()
      .help("Transcript density")
  }

  private var densityBinding: Binding<Int> {
      Binding(get: { model.density }, set: { model.setDensity($0) })
  }
  ```
  No keyboard shortcut carried over from Slint's `Ctrl+O` (`app.slint:729-732`) — it was tied to
  cycling semantics that no longer apply to a direct-selection dropdown.
- **`RowView.swift`**: `RowView` gains a `var forceToolExpanded: Bool = false` param (defaulted,
  so the file's own `#Preview` at the bottom needs no change), passed through to
  `case "tool": ToolRowView(row: row, forceExpanded: forceToolExpanded)`. `ToolRowView` gains a
  matching `let forceExpanded: Bool`, ORed into its existing detail-visibility check
  (`ToolRowView.swift`'s current `if expanded, !row.detail.isEmpty` becomes
  `if expanded || forceExpanded, !row.detail.isEmpty`) — its local `@State private var expanded`
  stays untouched, so a manual click still toggles independently. All four `RowView(row:)` call
  sites in `ChatView.swift` pass `forceToolExpanded: model.density == 0`; the parameter is inert
  for every non-`"tool"` kind, so passing it uniformly (including at the `thinking`-only
  `liveThinkingRows` site) is harmless and avoids special-casing individual call sites.

### Work breakdown

1. `AppModel.swift`: `density` (published, `UserDefaults`-backed) + `setDensity(_:)`; static
   `rowVisible(density:kind:)`.
2. `ChatView.swift`: wrap the four `ForEach` bodies with the `rowVisible` filter; add the
   density `Picker` (+ `densityBinding`); thread `forceToolExpanded: model.density == 0` into all
   four `RowView(row:)` call sites.
3. `RowView.swift`: `RowView`'s new `forceToolExpanded` param; `ToolRowView`'s new `forceExpanded`
   param ORed into its detail-visibility condition.
4. Verification.

**Status: implemented and committed** — commit `7927d77`, in the final `Picker` shape described
above (a cycle-button version was built first, then reworked into a direct-selection dropdown per
user follow-up, before either was committed). Confirmed working by the user.

### Acceptance criteria

- Clean `xcodebuild -project SwiftyPi.xcodeproj -scheme SwiftyPi build`.
- Manual: selecting a density from the toolbar dropdown, Verbose / Normal / Summary,
  visibly changes which rows show (Summary hides thinking/tool/info rows, including any *live*
  ones mid-turn — not just finalized rows) and whether tool-call rows render force-expanded
  (Verbose); a manual click on a tool row still toggles independently of the density-forced state;
  the chosen level persists across an app relaunch.

---

## Milestone SW8 — Composer completion: thinking-level control + session stats indicator

**Goal:** round out the composer/status-bar area SW6 started with the two pieces of state it
deliberately deferred — which thinking level is active (tied to model choice, refreshed at the
same points SW6 already refreshes model state) and a live session-size/cost indicator. Both
pull-or-push in the same shapes SW6 already established, no new architecture.

### Verified facts

- **`ThinkingLevel`** (`crates/pi-rpc/src/types.rs:17-27`): `Off | Minimal | Low | Medium | High |
  Xhigh | Max`, lowercase-serialized. `PiClient::set_thinking_level(level) -> Result<(), PiError>`
  (`client.rs:309-313`) already exists as a direct wrapper. `GetAvailableThinkingLevels` has **no**
  `PiClient` wrapper — only reachable via the generic `client.request(Command::
  GetAvailableThinkingLevels)`, exactly as `pi-core`'s own `refresh_thinking` calls it
  (`backend.rs:2677`).
- **`refresh_thinking`** (`backend.rs:2673-2712`): fetches available levels fresh every call (not
  cached/static — availability differs per model), builds `Vec<ThinkingLevel>` + `"think: {s}"`
  labels, separately calls `client.get_state()` to read `thinkingLevel` and locate its index among
  the just-fetched labels, pushes via `UiSink::set_thinking(labels, index)`. Called at session-loop
  start (`backend.rs:1197`, right after `refresh_models`) and after every successful `SetModel`
  (`backend.rs:1256-1267`, alongside `dot_interval.reset_immediately()`) — i.e. exactly the same
  two triggers SW6's `refresh_models` already uses.
- **`UiCmd::SetThinking(i)`** (`backend.rs:1269-1275`) indexes into the closure-captured
  `thinking_levels: Vec<ThinkingLevel>` from the last refresh, calls `client.set_thinking_level`.
  The ComboBox is gated on `thinking-list.length > 1` (`app.slint:986-990`) — hidden entirely for
  a model offering only one level.
- **`Command::GetSessionStats`** (`types.rs:91`) is parameterless; `PiClient::
  get_session_stats() -> Result<Value, PiError>` (`client.rs:315-319`) already exists as a direct
  wrapper. **`update_stats`** (`backend.rs:2714-2730`) reads response shape `{cost: f64, tokens:
  {total: u64}, contextUsage: {percent: f64}}`, computes `format_tokens(tokens.total)` (already
  `pub` at `pi_render`'s crate root, directly reusable) + `"{tokens} tok · ${cost:.4}"`, pushes via
  `set_status(String)`/`set_context_percent(f32)`.
- **Stats refresh triggers**: after every `Event::AgentSettled` (`backend.rs:1436`) and at the end
  of `hydrate_active_session` (`backend.rs:1550` — i.e. after switch/resume/clone/fork). No timer.
  **Wrinkle to design around, not port literally**: Slint's `set_status` slot is *also* reused for
  transient non-stats messages ("compacting context…", "retrying (n/m)…") that briefly override
  the token/cost string — the Swift app has no compaction/retry visibility at all yet (a separate,
  still-unbuilt gap matching M4's broader scope), so this milestone keeps stats in their own
  dedicated field rather than a shared, overloadable slot; a future compaction/retry-status feature
  gets its own slot later, not this one.

### Re-verified against the current codebase (post-SW6/SW7)

Facts re-checked directly against the live source (not just re-stated from the original draft):

- `ThinkingLevel` (`crates/pi-rpc/src/types.rs:17-27`), `Command::SetThinkingLevel{level}` (71-73),
  `Command::GetAvailableThinkingLevels` (74, bare), `Command::GetSessionStats` (91, bare) — all
  confirmed unchanged. `PiClient::set_thinking_level`/`get_session_stats`
  (`crates/pi-rpc/src/client.rs:309-320`) confirmed unchanged; still **no** `PiClient` wrapper for
  `GetAvailableThinkingLevels` (unlike `get_available_models`, which has one) — only reachable via
  `client.request(Command::GetAvailableThinkingLevels)`.
- `refresh_thinking`/`update_stats` (`crates/pi-core/src/backend.rs:2673-2712`/`2714-2730`)
  confirmed at those line numbers, same logic as originally recorded. One correction: the
  `UiCmd::SetThinking` handler (line 1269) does **not** re-call `refresh_thinking` after a
  successful `set_thinking_level` in the Slint reference — only `SetModel`'s handler does (line
  1262, since available levels can change per-model). The Swift design below still has
  `setThinkingLevel` re-fetch afterward regardless, matching this crate's own established
  after-action-refresh convention (`SW6`'s `setModel`) rather than a literal port of this one
  asymmetry — cheap, and keeps the checkmark correct with no extra risk.
- `pi_render::format_tokens` confirmed unchanged, `crates/pi-render/src/hydrate.rs:139-147`,
  re-exported at the crate root.
- **`crates/pi-core-ffi/src/lib.rs` (now 1606 lines) — confirmed current shapes to mirror**:
  `ChatCmd` (142-200) — `RefreshModels{reply: oneshot::Sender<Result<Vec<ModelRecord>, String>>}`/
  `SetModel{provider, model_id, reply: oneshot::Sender<Result<(), String>>}` (190-199) are the
  exact two shapes to mirror for `RefreshThinkingLevels`/`SetThinkingLevel`. `ChatSink` (77-132) —
  `on_server_dot_changed` (117) is the precedent for `on_session_stats_changed`. `PiSession::
  call(...)` (439-451) is **hardcoded to `Result<(), String>` replies** — usable directly for
  `set_thinking_level` (mirrors `set_model`'s own use of it, 409-420), but **not** usable for
  `refresh_thinking_levels` (needs a `Result<Vec<ThinkingLevelRecord>, String>` reply) — that one
  needs its own manually-built `oneshot::channel()`, exactly like `refresh_models` already does
  (396-403), not `self.call(...)`.
- **`hydrate_and_push`** (733-746) is called from **three** places already: launch-time resume
  (504), `SwitchSession` (593), and — critically — **from inside the `Event::AgentSettled` arm
  itself** (689-696, `hydrate_and_push(...)` is the first line of that arm). This means the
  original plan's "two trigger points" (AgentSettled *and* inside `hydrate_and_push`) were the
  same call graph counted twice — **the stats-push only needs to be inserted once, inside
  `hydrate_and_push` itself**, and it automatically covers all three triggers for free. Simpler
  than originally drafted.
- **`crates/pi-core-ffi/src/models.rs`** (305 lines, SW6) confirmed as the shape to mirror: one
  `uniffi::Record` output type (`ModelRecord`), one internal-only plain struct where a second
  computation needs extra fields (`ModelEntry`, for the server-dot), one `refresh_*` async fn doing
  fetch-and-map. Thinking levels don't need an equivalent internal struct — nothing else computes
  from them — so `thinking.rs` can be a notch simpler than `models.rs`, not a 1:1 structural copy.
- **Correction to the SwiftUI plan — the model picker isn't in the composer anymore.** Since this
  plan section was first drafted, a direct user request relocated SW6's model picker from
  `ChatView`'s composer to `SidebarView`'s footer (`SidebarView.swift`'s `.safeAreaInset(edge:
  .bottom)`, `modelPicker` at lines 120-140) — confirmed current. A thinking-level picker,
  being tied to the same "current model" state, belongs in the same place, not back in the
  composer as originally sketched. `ChatView.swift` (now 243 lines) has grown its own toolbar
  since (`densityPicker`, the streaming spinner) — `statusBar` (166-178) is still the right home
  for the stats ring/caption, unchanged from the original plan.

### Design

- **Thinking**: new `ThinkingLevelKind` (`uniffi::Enum` mirroring `pi_rpc::ThinkingLevel` 1:1) and
  `ThinkingLevelRecord{level: ThinkingLevelKind, label: String, is_current: bool}` (mirrors
  `ModelRecord`'s shape) in a new `crates/pi-core-ffi/src/thinking.rs`, reimplementing `refresh_
  thinking`'s logic (`client.request(GetAvailableThinkingLevels)` + `client.get_state()` to locate
  the current index, labels formatted `"think: {level}"`) as a free async fn returning
  `Vec<ThinkingLevelRecord>` directly (no `pi-core` dependency, matching this crate's posture
  throughout). New `ChatCmd::RefreshThinkingLevels{reply: oneshot::Sender<Result<Vec<
  ThinkingLevelRecord>, String>>}` (manual oneshot, per the note above) and `ChatCmd::
  SetThinkingLevel{level: ThinkingLevelKind, reply: oneshot::Sender<Result<(), String>>}` (via
  `self.call(...)`, per the note above); matching `run()` arms in the `cmd_rx.recv()` branch of the
  same `tokio::select!` `RefreshModels`/`SetModel` already live in. `PiSession::
  refresh_thinking_levels()`/`set_thinking_level(level:)` — both pull-based, no Rust-side
  auto-refresh (matches SW6 exactly): Swift's `setThinkingLevel` calls `refreshThinkingLevels()`
  afterward (mirroring `setModel`), `start()` calls `refreshThinkingLevels()` once alongside
  `refreshModels()`. `run_demo`/`reply_demo_action` get matching no-op/synthetic arms so demo mode
  keeps compiling and doesn't hang on the new commands.
- **Stats**: new `SessionStatsRecord{tokens_label: String, cost: f64, context_percent: f32}` in the
  same `thinking.rs` (one file per milestone, matching `models.rs`'s SW6 precedent of covering two
  related concerns together) — `tokens_label` is exactly `pi_render::format_tokens(tokens)` (e.g.
  `"1.2k"`), not the full `"{} tok · ${cost}"` caption — Swift composes the rest, keeping the
  formatting boundary at "Rust owns the token-count threshold logic, Swift owns simple
  interpolation." Push-based via a new `ChatSink::on_session_stats_changed(stats:
  SessionStatsRecord)`, inserted **once**, inside `hydrate_and_push` itself (see the correction
  above — this single insertion point already covers resume, `SwitchSession`, and every
  `AgentSettled`, since all three already funnel through this one function).
- **SwiftUI**: a `thinkingLevelPicker` in `SidebarView.swift`, placed in the same
  `.safeAreaInset(edge: .bottom)` footer as `modelPicker` (a second `Divider()` + padded block
  below it), gated on `model.thinkingLevels.count > 1` (hidden entirely for a single-level model,
  matching Slint's `thinking-list.length > 1` gate) — same `Menu`/checkmark-prefix-`Text`/
  offset-keyed-`ForEach` shape as `modelPicker`, just with a `"brain"` SF Symbol instead of `"cpu"`.
  In `ChatView.swift`'s `statusBar` (166-178): a small usage ring (`Circle().trim(from:0,to:)` +
  `.rotationEffect(.degrees(-90))`, red past 85%, mirroring Slint's clockwise-from-12-o'clock donut
  semantics) plus the composed `"{tokensLabel} tok · ${cost}"` caption, in a leading `HStack`
  segment ahead of the existing `Spacer()` + red `statusMessage` caption — the two captions never
  collide since they occupy opposite ends of the same bar.

### Work breakdown

1. `pi-core-ffi`: new `thinking.rs` (`ThinkingLevelKind`, `ThinkingLevelRecord`,
   `refresh_thinking_levels(client) -> Vec<ThinkingLevelRecord>`, `SessionStatsRecord`,
   `fetch_session_stats(client) -> Option<SessionStatsRecord>`); `ChatCmd::
   RefreshThinkingLevels`/`SetThinkingLevel`; matching `run()` arms; `PiSession::
   refresh_thinking_levels`/`set_thinking_level`; `ChatSink::on_session_stats_changed`;
   `hydrate_and_push` gains the one `fetch_session_stats` + push call; `run_demo`/
   `reply_demo_action` gain no-op arms for the two new commands. Unit tests: level-list/
   current-index matching and `SessionStatsRecord` formatting (mirroring `models.rs`'s own test
   style, `crates/pi-core-ffi/src/models.rs:213-305`).
2. Regenerate bindings (`scripts/build-rust.sh`), `swiftc -typecheck`.
3. `AppModel.swift`: `thinkingLevels`/`sessionStats` published state, `refreshThinkingLevels()`/
   `setThinkingLevel(_:)` (mirroring `refreshModels()`/`setModel(provider:modelId:)`,
   `AppModel.swift:379-399`), `onSessionStatsChanged` (mirroring `onServerDotChanged`,
   `AppModel.swift:547-551`); wire `refreshThinkingLevels()` into `start()` (alongside the existing
   `refreshModels()` call) and into `setModel()`.
4. `SidebarView.swift`: `thinkingLevelPicker` in the `.safeAreaInset` footer alongside
   `modelPicker` (120-140).
5. `ChatView.swift`: usage ring + stats caption in `statusBar` (166-178).
6. `spike_check.rs`: extend with a thinking-level list/set round trip and a stats fetch after a
   real turn.
7. Verification.

### Risks

- `GetAvailableThinkingLevels`/`GetSessionStats` have no typed response anywhere (same raw-`Value`
  posture as `GetAvailableModels`/`GetState`) — tolerant parsing, degrades gracefully on a shape
  change, matching this crate's established posture throughout.
- **`contextUsage.percent`'s scale (0-1 fraction vs. 0-100 percent) isn't confirmed** — the
  verified facts only established the field's JSON path and that it's read as `f64`, not its
  actual observed range against a real session. Default assumption for implementation: treat it as
  0-100 (matching the field's own name) and divide by 100 for the ring's `trim(from:0,to:)`
  fraction — confirm against a real `get_session_stats()` response during verification and adjust
  if wrong (a one-line fix either way).
- SwiftUI's `Circle().trim`/`.rotationEffect` ring math needs to actually match Slint's sweep
  direction/red-past-85% threshold visually — a standard, low-risk SwiftUI pattern, but worth a
  side-by-side glance during manual verification.

### Acceptance criteria

- `cargo test --workspace` green (new `thinking.rs` tests); clean
  `xcodebuild -project SwiftyPi.xcodeproj -scheme SwiftyPi build`. Real `pi` verification: the
  thinking-level picker in the sidebar footer appears only for models offering more than one
  level, and selecting one sticks; the usage ring and stats caption in the chat pane's status bar
  update after a real turn settles and after switching sessions, matching the numbers
  `get_session_stats` actually returns.

---

## Milestone SW9 — Attach files/images to prompts

**Goal:** let the user attach images (sent inline with the next prompt) or arbitrary files
(appended as an `@path` reference) from the composer — via a picker button and native
drag-and-drop, mirroring Slint's "attach" toolbar chip.

### Verified facts (re-checked against the current codebase, post-SW8)

**The Slint-side reference is already fully implemented and proven — not just designed.** This
milestone is purely additive to `pi-core-ffi`/Swift; nothing on the `pi-core`/Slint side needs to
change.

- **`crates/pi-core/src/attach.rs`** (70 lines, unchanged shape from the original draft):
  ```rust
  pub fn image_mime_type(path: &Path) -> Option<&'static str> {
      let ext = path.extension()?.to_str()?.to_ascii_lowercase();
      Some(match ext.as_str() {
          "png" => "image/png", "jpg" | "jpeg" => "image/jpeg", "gif" => "image/gif",
          "webp" => "image/webp", "bmp" => "image/bmp", _ => return None,
      })
  }
  pub fn encode_base64(bytes: &[u8]) -> String {
      base64::engine::general_purpose::STANDARD.encode(bytes)
  }
  ```
  Both have their own unit tests already (lines 38-70) — mirror these in the new `pi-core-ffi`
  copy per this crate's established port-with-tests convention (`models.rs`/`thinking.rs`).
- **`crates/pi-core/src/backend.rs`'s `attach_path` handler** (lines 1561-1602, full body
  confirmed): classifies via `attach::image_mime_type`; non-image → `transcript.ui.
  append_composer_text(&path)` (an `@path` reference) and returns early; image → `tokio::fs::
  read`, `attach::encode_base64`, pushes `(display_name, ImageContent)` into `pending_images:
  Vec<(String, ImageContent)>` (declared line 1208, session-loop-scoped), then `transcript.ui.
  set_pending_attachments(names)`. `UiCmd::Send`'s arm (~1216-1234) drains `pending_images` via
  `std::mem::take` into `Vec<ImageContent>`, clears the chip row, and calls
  `client.prompt_with_images(&text, images).await` — but only for **non-streaming** sends; a
  mid-stream send goes through `client.prompt_steering(&text)` (no images param) instead, leaving
  attachments queued for the next non-streaming send. `UiCmd::RemoveAttachment(index)`
  (~1241-1247) removes by index and re-pushes the chip-name list.
- **`ImageContent`** (`crates/pi-rpc/src/types.rs:29-35`, already `pub`, already used in
  `pi-core/backend.rs`):
  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize)]
  #[serde(rename_all = "camelCase")]
  pub struct ImageContent { #[serde(rename = "type")] pub kind: String, pub data: String, pub mime_type: String }
  ```
- **`PiClient::prompt`/`prompt_with_images`** (`crates/pi-rpc/src/client.rs:244`/`255`) both
  confirmed current — `prompt_with_images(message, images: Vec<ImageContent>)` wraps
  `Command::Prompt{message, images: Some(images), ..}`.
- **`crates/pi-core-ffi/src/lib.rs` (1687 lines) has zero attachment-related code today**
  (confirmed via grep) — `ChatCmd::Send`'s current arm (lines 561-566) is exactly
  `if let Err(e) = client.prompt(text).await { report_error(...) }`, no images, no branching, and
  no `prompt_steering`/mid-stream distinction exists in this crate at all (an existing,
  unrelated simplification already accepted here — this milestone doesn't need to introduce that
  distinction just to add attachments; every `Send` in this crate is a plain `client.prompt(...)`
  call today, so the only change needed is making *that one call* conditionally use
  `prompt_with_images`). `ChatSink` (lines 80-141, last method `on_session_stats_changed` from
  SW8) and `ChatCmd` (starts line 151, last variants `RefreshThinkingLevels`/`SetThinkingLevel`
  from SW8) both need two new additions each. `reply_demo_action` (`fn` at line 965) has **no
  wildcard arm** — it's fully exhaustive over `ChatCmd`, so both new variants need explicit arms
  there or the crate won't compile. `test_support::RecordingSink` (lines 1279-1350) and
  `examples/spike_check.rs`'s `PrintSink` (its own `impl ChatSink`) both also need the two new
  trait methods implemented to keep compiling.
- **No `base64` dependency in `pi-core-ffi/Cargo.toml` yet** (confirmed via grep) — `pi-core`
  pins `base64 = "0.23.0"`; the new `pi-core-ffi/src/attach.rs` needs the same dependency added.
- **Swift side confirmed unchanged in the relevant ways**: `draft: String` is still
  `@State private var draft` local to `ChatView.swift:12`, not on `AppModel` — the
  `pendingComposerAppend` mechanism the original plan sketched is still necessary. The composer
  `HStack` (`ChatView.swift:60-82`, `TextField` + Send/Abort) is immediately followed by
  `.padding(12)` then `statusBar` (moved below the composer per a recent direct request) — an
  attachment-chip row's natural slot is between those two, i.e. right after the composer `HStack`
  and before `statusBar`. Zero existing drag-and-drop code anywhere in the Swift app (`grep` for
  `dropDestination`/`onDrop`/`NSItemProvider` — no hits) — the only existing `NSOpenPanel` usage
  is `AppModel.promptSwitchProject()` (unrelated, a directory picker for switching projects).
  `AppModel`'s `ChatSink` conformance section (`// MARK: - ChatSink`, `AppModel.swift:519`) has an
  established one-method-per-push-signal pattern to mirror exactly (e.g. `onServerDotChanged`,
  line 584: `nonisolated func onServerDotChanged(state: ServerDotState) { Task { @MainActor in
  self.serverDot = state } }`).

### Design

- New `pi-core-ffi/src/attach.rs`: ported `image_mime_type`/`encode_base64` (verbatim from
  `pi-core`, plus their existing tests) and a new async `attach_path` helper taking
  `pending_images: &mut Vec<(String, ImageContent)>`, `path: &Path`, `sink: &dyn ChatSink` —
  mirrors `pi-core::backend::attach_path`'s exact logic (classify → read+encode+queue+push chips,
  or push an `@path` composer-append signal), just replacing `transcript.ui.*` calls with the two
  new `ChatSink` pushes below.
- Two new `ChatSink` methods (added after `on_session_stats_changed`, following the same
  push-based pattern as `on_server_dot_changed`/`on_thinking_row_changed` — inherently event-driven
  by attach/remove actions, not polled state, so pull doesn't fit here):
  ```rust
  fn on_pending_attachments_changed(&self, names: Vec<String>);
  fn on_composer_append(&self, text: String);
  ```
  `on_composer_append` is the first concrete, justified use of the "push text into the composer"
  pattern previously only flagged as a hypothetical during SW5's parked `set_editor_text`
  research — SW10's fork-prefill can reuse the same mechanism later.
- Two new `ChatCmd` variants, **fire-and-forget** (no reply channel), matching `Send`/`Abort`'s
  shape exactly rather than the reply-carrying session-lifecycle actions — mirrors Slint's own
  fire-and-forget `UiCmd::AttachPath`/`RemoveAttachment`:
  ```rust
  AttachPath { path: String },
  RemoveAttachment { index: usize },
  ```
  `run()` gains `let mut pending_images: Vec<(String, ImageContent)> = Vec::new();` (loop-scoped
  mutable local, same pattern as `managed_rapid_mlx`/`live_thinking`) and two new match arms:
  ```rust
  Some(ChatCmd::AttachPath { path }) => {
      attach::attach_path(&mut pending_images, Path::new(&path), sink.as_ref()).await;
  }
  Some(ChatCmd::RemoveAttachment { index }) => {
      if index < pending_images.len() {
          pending_images.remove(index);
          sink.on_pending_attachments_changed(pending_images.iter().map(|(n, _)| n.clone()).collect());
      }
  }
  ```
  `ChatCmd::Send`'s arm becomes conditional on whether `pending_images` is non-empty:
  ```rust
  Some(ChatCmd::Send(text)) => {
      let images: Vec<ImageContent> = std::mem::take(&mut pending_images).into_iter().map(|(_, img)| img).collect();
      if !images.is_empty() { sink.on_pending_attachments_changed(Vec::new()); }
      let result = if images.is_empty() { client.prompt(text).await } else { client.prompt_with_images(text, images).await };
      if let Err(e) = result { report_error(sink.as_ref(), describe(&e)); }
  }
  ```
- `PiSession` gains two plain, non-async, fire-and-forget methods (same block as `send`/`abort`,
  not the `async_runtime = "tokio"` block):
  ```rust
  pub fn attach_path(&self, path: String) { let _ = self.cmd_tx.send(ChatCmd::AttachPath { path }); }
  pub fn remove_attachment(&self, index: u32) { let _ = self.cmd_tx.send(ChatCmd::RemoveAttachment { index: index as usize }); }
  ```
  (`u32` at the FFI boundary, not `usize` — UniFFI doesn't export `usize`/`isize` directly.)
  `reply_demo_action` gets two no-op arms (mirroring `ChatCmd::ReplyExtensionDialog`'s existing
  `{} // no-op` treatment): demo mode has no real files to attach and never queues any.
- Image bytes never reach Swift — `pending_images`' base64 data stays entirely Rust-side (queued
  in `run()`, consumed by the next `Send`); Swift only ever needs the *names* for the chip row,
  matching `set_pending_attachments`'s names-only shape exactly.
- **SwiftUI**: `AppModel` gains `private(set) var pendingAttachments: [String] = []`,
  `private(set) var pendingComposerAppend: String?`, `func attachPath(_ path: String)`/
  `func removeAttachment(at index: Int)` (plain, non-`async`, matching `PiSession`'s own
  fire-and-forget shape), `func consumePendingComposerAppend() -> String?` (reads and clears in
  one call — avoids a re-render race between reading the value and clearing it), and the two new
  `ChatSink` conformance methods (`onPendingAttachmentsChanged`/`onComposerAppend`, mirroring
  `onServerDotChanged`'s one-line-assignment shape). `ChatView` gains: a paperclip `Button`
  (leading edge of the composer `HStack`, opening an `NSOpenPanel` — multi-select, `canChooseFiles
  = true`, no content-type restriction since non-images route differently — calling
  `model.attachPath(path:)` per selected URL; needs `import AppKit`, matching the established
  precedent in `RowView.swift`/`AppModel.swift`); a small attachment-chip row (name + "×" per
  chip calling `model.removeAttachment(at:)`) between the composer `HStack` and `statusBar`; a
  `.onChange(of: model.pendingComposerAppend)` that calls `consumePendingComposerAppend()` and
  appends the result into local `draft`; `.dropDestination(for: URL.self)` on the composer area
  routing dropped file URLs through the same `attachPath` call.

### Work breakdown

1. `pi-core-ffi/Cargo.toml`: add `base64 = "0.23.0"` (matching `pi-core`'s pin).
2. `pi-core-ffi`: new `attach.rs` (ported `image_mime_type`/`encode_base64` + their tests, new
   `attach_path` helper); `ChatCmd::AttachPath`/`RemoveAttachment`; `run()`'s `pending_images`
   state + two new arms; `Send`'s arm updated to branch on `prompt_with_images`; `ChatSink::
   on_pending_attachments_changed`/`on_composer_append`; `PiSession::attach_path`/
   `remove_attachment`; `reply_demo_action`'s two new no-op arms; `test_support::RecordingSink`
   and `spike_check.rs`'s `PrintSink` gain the two new trait methods.
3. Regenerate bindings (`scripts/build-rust.sh`), `swiftc -typecheck`.
4. `AppModel`: `pendingAttachments`/`pendingComposerAppend` state, `attachPath`/`removeAttachment`/
   `consumePendingComposerAppend` methods, `ChatSink` conformance.
5. `ChatView`: paperclip button + `NSOpenPanel` in the composer `HStack`; attachment-chip row;
   `.onChange` consuming `pendingComposerAppend` into local `draft`; `.dropDestination` wiring.
6. `spike_check.rs`: extend with an attach-path round trip against a small fixture image,
   confirming the chip-name push fires and a subsequent send doesn't error.
7. Verification.

### Risks

- SwiftUI's exact `.dropDestination`/`UTType` API shape for file URLs needs confirming during
  implementation — not yet exercised anywhere in this app, though the deployment target (macOS
  14+) comfortably covers the API's macOS 13+ minimum.
- Dragging a non-file payload (e.g. a folder, or dragged text) needs graceful handling — the
  Rust-side `image_mime_type` returning `None` falls through to `@path`-append regardless of
  whether the path is even a regular file; Swift should add a defensive regular-file check before
  calling `attachPath` that the Rust reference itself doesn't bother with, since drag sources are
  more varied on macOS than a plain file-picker selection.
- No mid-stream `prompt_steering` distinction exists in `pi-core-ffi` today (Verified facts) — a
  `Send` fired while already streaming will still just call `client.prompt(...)`/
  `prompt_with_images(...)` as today, not queue as a steering message. Out of scope for this
  milestone (a pre-existing gap, not introduced here); flagged so it isn't mistaken for something
  this fix was supposed to also handle.

### Acceptance criteria

- `cargo test --workspace` green (new `attach.rs` tests); clean
  `xcodebuild -project SwiftyPi.xcodeproj -scheme SwiftyPi build`. Real `pi` verification:
  attaching a real image via the picker shows a chip and a follow-up prompt (e.g. "describe this
  image") reflects it; attaching a non-image file appends `@path` text to the composer;
  drag-and-drop from Finder works via the native SwiftUI mechanism; removing a chip before sending
  drops that attachment.

---

## Milestone SW10 — Session branch tree + fork-from

**Goal:** let the user see a session's full branch history and fork an earlier user message into a
new branch, mirroring Slint's "tree" overlay.

### Verified facts

- **Toolbar "tree" chip**, **Cmd+T** open, **Esc** close (`app.slint:965-968`, `721-724`,
  `698-701`). **`UiCmd::OpenTree` / `ForkFrom(String)`** (`backend.rs:69-72`).
- **`Command::GetTree`** → `client.get_tree()` (`pi-rpc/src/client.rs:369-371`) — response
  `{leafId: string, tree: [...]}`, a recursive node tree. **`fetch_tree_rows`** (`backend.rs:
  2200-2232`) flattens via **`flatten_tree`** (`2234-2268`, computing depth/summary/label/can_fork
  per node) and **`tree_node_summary`** (`2270+`, formats a one-line summary per entry kind — user
  text, `"assistant: ..."` prefix, `"→ {toolName}"` for a `toolResult`, etc.); `can_fork = entry.
  type == "message" && message.role == "user"` — only user messages are forkable. The active-branch
  node set is computed by walking parent links from `leafId` back to the root.
- **`UiSink::set_tree(rows: Vec<(id, depth, summary, label, can_fork, active)>)`.**
  **`TreeOverlay`/`TreeNodeItem`** (`crates/slinty-pi/ui/tree.slint`, full file): a scrim-dismiss
  modal, indented rows (bullet marker for active), a double-tap-confirm "Fork" button per forkable
  row (same 2.5s pattern as delete).
- **`fork_from`** (`backend.rs:1605-1631`): `Command::Fork{entry_id}` → `client.fork()`
  (`client.rs:349-356`) rewinds the active branch to *before* that message and returns its
  original text (not kept in context after the rewind); the handler then calls `hydrate_active_
  session` (full reload) and pre-fills the composer via `set_composer_text`, so the user doesn't
  lose the prompt they meant to redo.
### Re-verified against the current codebase (post-SW6–SW9), including the open question's resolution

**Resolved: do not reuse `pi_sessions::tree::SessionTree` — port `flatten_tree`/`tree_node_summary`
fresh, against the live RPC, same as `pi-core`'s own reference.** Full comparison:

- `pi_sessions::tree::SessionTree` (`crates/pi-sessions/src/tree.rs`, 131 lines) reads one session's
  JSONL file straight off disk and exposes raw `entries: Vec<SessionEntry>` (id/parent_id/
  timestamp/kind only) plus `get`/`children_of`/`active_branch`. It computes **none** of what the
  overlay needs — no `depth`, no human-readable `summary`, no `label` resolution, no `can_fork`, no
  per-row `is_active` flag. Reproducing those on top of it would mean re-deriving `flatten_tree`/
  `tree_node_summary`'s logic anyway, for zero savings.
- Its own doc comment (lines 16-24) **explicitly warns it's unsafe for this exact use case**: its
  `leaf_id` is "best-effort — the last entry appended to the file... can be wrong for a session
  closed right after `/tree`-jumping or forking to an earlier point with nothing sent afterward...
  A *running* session's true leaf is authoritative via RPC (`get_tree`/`get_entries` both return
  `leafId`); prefer that when a session is live." SW10's tree overlay is opened against exactly a
  *live* session — using `SessionTree` here would silently mis-highlight the active branch after a
  fork/tree-jump, a real bug the author already flagged, not a hypothetical one.
- Repo-wide grep confirms `SessionTree` has exactly one real consumer today:
  `pi_core::demo_sessions::hydrate_messages` (`crates/pi-core/src/demo_sessions.rs:57`), for
  `SLINTY_DEMO=1`'s fake sidebar (no live `pi` child exists there, so there's nothing to RPC
  against) — it was never wired into a real tree/fork UI, live or offline. Leave it exactly as-is;
  it's the right tool for its one existing job, just not this one.
- **Port target confirmed current**: `crates/pi-core/src/backend.rs`'s `FlatTreeRow` struct/
  `fetch_tree_rows`/`flatten_tree`/`tree_node_summary` (lines 2190-2351) and `fork_from`
  (1607-1631) are all still present at those line numbers. `fetch_tree_rows` calls `client.
  get_tree()`, reads the *live* `leafId`, and returns `Vec<(id, depth, summary, label, can_fork,
  is_active_branch)>` for the whole tree; `flatten_tree` recursively walks `get_tree`'s nested
  `{entry, children, label?}` JSON; `tree_node_summary` is a full match over every session-entry
  `type` (message/model_change/thinking_level_change/compaction/branch_summary/custom/
  custom_message/session_info), reusing `pi_render::user_content_text`/`first_line` — both already
  available to `pi-core-ffi` (it depends on `pi-render` already, no new dependency needed).
  `fork_from` calls `client.fork(entry_id)`, hydrates via `hydrate_active_session`, and pre-fills
  the composer via `transcript.ui.set_composer_text(text)` — a **replace**, not an append.
- `PiClient::fork`/`get_tree()` (`crates/pi-rpc/src/client.rs:347-356`/`369-371`) and
  `Command::Fork{entry_id}`/`Command::GetTree` (`crates/pi-rpc/src/types.rs:102-104`/`111`) all
  confirmed current, unchanged signatures.
- **`crates/pi-core-ffi/src/lib.rs` (1768 lines) has zero tree/fork code today** — its own crate
  doc comment (lines 5-6) explicitly says `ChatSink` "stays smaller than `pi_core::backend`'s full
  `UiSink` (no palette/tree surface)" — SW10 is what finally closes that gap. Current `ChatCmd`
  (lines 163-243, 15 variants ending with SW9's `AttachPath`/`RemoveAttachment`) and `ChatSink`
  (lines 81-153, 14 methods ending with SW9's `on_composer_append`) both need new additions.
  `PiSession::call(...)` (lines 526-538) is still hardcoded to `Result<(), String>` replies — fine
  for `fork_from`, but `open_tree` needs a `Result<Vec<TreeRowRecord>, String>` reply, so it needs
  its own hand-rolled oneshot, exactly like `refresh_models`/`refresh_thinking_levels` already do
  (lines 456-463/487-498) rather than `call`.
- **Confirmed: SW9's `on_composer_append`/`pendingComposerAppend` cannot be reused for fork's
  prefill — it's hardcoded to append-with-`@`-prefix.** `attach.rs:48` calls `sink.
  on_composer_append(path.display().to_string())`; `ChatView.swift`'s `.onChange` handler
  (lines 105-108) does `draft += draft.isEmpty ? "@\(text)" : " @\(text)"` — baked-in `@` and
  append semantics matching only the attach use case. Fork needs a genuine *replace* (`pi_core::
  backend::fork_from`'s `set_composer_text`, no `@`, no append) — reusing the existing mechanism
  as-is would corrupt the prefill by gluing a `@`-prefixed forked message onto whatever's already
  typed. A new, single-purpose `ChatSink::on_composer_replace(text: String)` is the right shape,
  consistent with every other `ChatSink` method already being single-purpose.
- **Swift toolbar placement confirmed**: a tree button belongs in `ChatView.swift`'s toolbar
  (lines 112-146, currently status dots + density picker — content-view-scoped, per-active-session
  controls), not `SidebarView.swift`'s (lines 55-81 — project/session-*lifecycle* actions: New
  Session/Switch Project/Models). Mirror the density picker's `#available(macOS 26.0, *)` /
  `.sharedBackgroundVisibility(.hidden)` `ToolbarItem` pattern exactly. `ExtensionDialogView.swift`'s
  `SelectDialogView` (lines 88-117) is the exact `.sheet` skeleton to mirror: `NavigationStack` +
  `List` + `.navigationTitle` + a `ToolbarItem(placement: .cancellationAction)` Close button +
  `.frame(minWidth:minHeight:)` + `.interactiveDismissDisabled()`.

### Design

- New `pi-core-ffi/src/tree.rs`: `TreeRowRecord{id: String, depth: i32, summary: String, label:
  String, can_fork: bool, is_active: bool}` (`uniffi::Record`, mirrors the ported tuple 1:1) plus
  ported `flatten_tree`/`tree_node_summary`/a `fetch_tree_rows(client: &PiClient) ->
  Vec<TreeRowRecord>` — verbatim logic from `pi_core::backend`, adapted to return
  `TreeRowRecord`s instead of tuples and pushing nothing itself (pure fetch-and-map, matching
  `models.rs`/`thinking.rs`'s established shape).
- Two new `ChatCmd` variants:
  ```rust
  OpenTree { reply: oneshot::Sender<Result<Vec<TreeRowRecord>, String>> },
  ForkFrom { entry_id: String, reply: oneshot::Sender<Result<(), String>> },
  ```
  `run()` gains two matching arms: `OpenTree` calls `tree::fetch_tree_rows(&client).await` and
  replies `Ok(rows)` (hand-rolled oneshot, per the note above — not `self.call`). `ForkFrom` calls
  `client.fork(&entry_id).await`; on success, extracts the returned prompt text, calls the existing
  `hydrate_and_push(&client, sink.as_ref(), &dark).await` (re-renders the now-active branch — same
  hook point SW8/SW9 both used), then `sink.on_composer_replace(text)`, then replies `Ok(())`.
  `reply_demo_action` gets two new arms (its match is exhaustive, no wildcard — confirmed still
  true): `OpenTree` replies `Ok(Vec::new())` (no branch structure in demo mode), `ForkFrom` replies
  `Ok(())` with no-op (mirrors `AttachPath`/`RemoveAttachment`'s SW9 precedent).
- New `ChatSink::on_composer_replace(text: String)` — single-purpose, added after
  `on_composer_append`, **not** a mode flag on the existing method (keeps every `ChatSink` method
  single-purpose, matching the established style).
- `PiSession::open_tree() -> Result<Vec<TreeRowRecord>, PiSessionError>` (hand-rolled oneshot, pull,
  called when the sheet opens — mirrors `refresh_models`) and `PiSession::fork_from(entry_id:
  String) -> Result<(), PiSessionError>` (via `self.call(...)`, reply is `Result<(), String>`).
- **SwiftUI**: `AppModel` gains `private(set) var treeRows: [TreeRowRecord] = []`,
  `private(set) var pendingComposerReplace: String?`, `func openTree() async`, `func
  forkFrom(entryId:) async` (calls `session.forkFrom`, then relies on the push-based
  `onComposerReplace`/`onHistoryReplaced` to update state — no manual refetch needed, matching how
  SW9's attach flow already works), `consumePendingComposerReplace() -> String?` (same
  read-and-clear-atomically shape as SW9's `consumePendingComposerAppend`), and a new
  `nonisolated func onComposerReplace(text:)` conformance method (mirroring `onComposerAppend`'s
  one-line-assignment shape — `onHistoryReplaced` already exists from earlier milestones and needs
  no changes). New `TreeView.swift` sheet (mirrors
  `ExtensionDialogView.swift`'s `SelectDialogView` skeleton): a `List` of `treeRows`, each row
  indented `.padding(.leading, CGFloat(row.depth) * 16)`, a filled/hollow circle marker for
  `isActive`, and a "Fork" button shown only when `canFork` — tapping it calls
  `model.forkFrom(entryId:)` then dismisses the sheet. `ChatView.swift` gains a new toolbar button
  (mirroring the density picker's `#available`/`.sharedBackgroundVisibility` pattern exactly),
  `.sheet(isPresented:)` presenting `TreeView`, `.keyboardShortcut("t")`, and a new
  `.onChange(of: model.pendingComposerReplace)` that calls `consumePendingComposerReplace()` and
  sets `draft = text` directly (a genuine replace, unlike SW9's append handler).
- **Fork confirmation**: a plain `.alert` before calling `forkFrom`, not Slint's 2.5s double-tap
  chip — consistent with the delete-confirmation precedent already established this session (native
  idiom over literal port).

### Work breakdown

1. `pi-core-ffi`: new `tree.rs` (`TreeRowRecord`, ported `flatten_tree`/`tree_node_summary`/
   `fetch_tree_rows`); `ChatCmd::OpenTree`/`ForkFrom` + `run()`'s two new arms;
   `ChatSink::on_composer_replace`; `PiSession::open_tree`/`fork_from`; `reply_demo_action`'s two
   new arms; `test_support::RecordingSink` and `spike_check.rs`'s `PrintSink` gain the one new
   trait method. Unit tests: `tree_node_summary`'s per-entry-type formatting (mirroring
   `models.rs`/`thinking.rs`'s test style).
2. Regenerate bindings (`scripts/build-rust.sh`), `swiftc -typecheck`.
3. `AppModel`: `treeRows`/`pendingComposerReplace` state, `openTree()`/`forkFrom(entryId:)`/
   `consumePendingComposerReplace()`, `onComposerReplace` conformance.
4. New `TreeView.swift` sheet; `ChatView.swift` toolbar button + `.sheet` + `.keyboardShortcut("t")`
   + `.onChange` wiring (with a confirmation `.alert` before forking).
5. `spike_check.rs`: extend with a `tree` mode — fetch the tree, fork from an earlier user message
   on a real multi-turn session, confirm `on_composer_replace` fires with the original text and
   history re-hydrates to the new active branch.
6. Verification.

### Risks

- `Command::Fork`'s exact response shape (`{text, cancelled?}`) is untyped `Value` like most
  commands here — tolerant parsing, matches this crate's established posture throughout.
- `tree_node_summary`'s full match over every session-entry `type` is the largest single function
  ported in this milestone — port it verbatim rather than simplifying, since any behavioral drift
  from the Slint reference would show up as confusing/wrong summary text in the overlay.

### Acceptance criteria

- `cargo test --workspace` green (new `tree.rs` tests); clean
  `xcodebuild -project SwiftyPi.xcodeproj -scheme SwiftyPi build`. Real `pi` verification against a
  session with at least one prior fork/branch: the tree sheet shows the correct flattened structure
  with the right node marked active; forking from an older user message rewinds the session,
  reloads history, and *replaces* (not appends to) the composer with that message's original text.

---

## Fix — Export session to markdown

**Status: parked at the user's request** (2026-08-12) — deprioritized behind SW10, not dropped.
Re-pick-up whenever the user wants it back on the active list; the plan below is still accurate as
of when it was written.

**Goal:** let the user save a whole session's transcript as a `.md` file. Genuinely new territory
— no existing wire command or formatting function does this today, confirmed by research.

### Verified facts

- **`Command::ExportHtml{output_path: Option<String>}`** (`pi-rpc/src/types.rs:92-96`) exists but is
  **completely unused anywhere in this codebase** — zero call sites in `pi-core`/`pi-core-ffi`/
  `slinty-pi`. Listed in `PRODUCT_PLAN.md`/`docs/plans/M5-ship.md` as unbuilt M5 scope; its actual
  response behavior (returns HTML as a string vs. pi writing a file directly) is undocumented in
  this repo (lives in upstream pi-coding-agent, not vendored here) — not something to build on
  blind for a *markdown* (not HTML) export anyway.
- **No markdown-flavored export capability exists anywhere**, confirmed by exhaustive grep:
  `pi-render`'s `hydrate_rowspecs`/`segment_markdown` go the opposite direction (markdown/JSON →
  structured rows for rendering); `pi-sessions` has no formatting-to-text functions at all.

### Design

- Build entirely client-side in Rust — no new wire command, since `ExportHtml`'s real behavior is
  unverified and markdown is what's wanted anyway. New `pi_render::export_markdown(messages:
  &[serde_json::Value]) -> String` (sibling to `hydrate_rowspecs`, same input shape), walking the
  raw message array directly rather than round-tripping through `RowSpec`/`RowRecord` (which have
  already lost some fidelity — e.g. `raw` is only populated for a prose group's first row) — more
  faithful to a full-session export. Reuses `tool_summary`/`content_text`/`user_content_text` for
  formatting consistency with the rendered UI, but keeps code fences intact (unlike the copy
  button's fence-free `.text`) since a full export should be a faithful markdown document.
- New `PiSession::export_markdown() -> Result<String, PiSessionError>` — pull, on-demand, async;
  fetches `get_messages()` fresh then calls `pi_render::export_markdown`, mirroring `hydrate_and_
  push`'s own fetch pattern but returning a string instead of pushing rows.
- **SwiftUI**: a toolbar/menu "Export…" action calling `exportMarkdown()`, presenting a native
  `NSSavePanel` (`.md` default extension) and writing the returned string to the chosen file — no
  new `ChatCmd`/`ChatSink` needed, a one-shot pull-and-write.

### Work breakdown

1. `pi-render`: new `export_markdown` function + fixture-based unit tests (mirroring `hydrate_
   tests`' style — a small synthetic message array covering prose/code/tool/thinking/table).
2. `pi-core-ffi`: `PiSession::export_markdown()`.
3. Regenerate bindings, `swiftc -typecheck`.
4. `AppModel`: `exportMarkdown()` method.
5. SwiftUI: toolbar "Export…" action + `NSSavePanel` wiring (alongside the existing "Switch
   Project"/"Models" toolbar buttons).
6. `spike_check.rs`: extend with an export round trip against a real multi-turn session, printing
   the resulting markdown's length/first lines.
7. Verification.

### Risks

- No existing format convention to match (unlike every other item in this plan, which mirrors a
  working Slint reference) — genuinely new design surface. Keep the markdown shape simple and
  readable (headings per turn/role, fenced code blocks, tool calls as a labeled sub-section) rather
  than over-engineering a format nobody's validated yet; easy to iterate on after real use.

### Acceptance criteria

- `cargo test --workspace` green (new `pi-render` tests); clean `xcodebuild`. Real `pi`
  verification: exporting a real multi-turn session (including at least one code block and one
  tool call) produces a readable, valid markdown file when opened.

---

## Fix — Current-project visibility + File-menu previous-projects switcher

**Goal:** asked directly — "there is no way of seeing what the current project folder is or
seeing past projects in other folders. Is the historical data available in the backend and
exposed? What kind of UI could we provide? Maybe 'previous projects' in the File menu? At the
very least the path to the current project should be visible somewhere, maybe in the navigation
bar where it currently just says 'pi'." Two related gaps: the chat pane's title bar shows a
hardcoded, meaningless string regardless of which project is open, and there's no way to jump
back to a previously-used project short of re-navigating a folder picker from scratch — despite
the backend already having (and Swift already fetching) exactly the data needed for both.

### Verified facts

- **The "previous projects" data already exists and is already wired to Swift — just never read
  by any view.** `pi_sessions::list_projects(root)` (`crates/pi-sessions/src/scan.rs:99-118`)
  scans `~/.pi/agent/sessions` and returns every known project as `Project{dir_name, display_path,
  session_dir}`, sorted alphabetically by `display_path`. `display_path` is the real recorded
  `cwd` from a session's header when one exists (`first_session_cwd`, line 120), only falling back
  to a lossy dash-decode (`decode_project_dir`) for a project directory with zero readable session
  files — a case `list_projects` can't actually produce, since a project only gets a directory
  once pi has written at least one session into it. This is already wrapped end-to-end for Swift:
  `SessionIndex::list_projects()` (`crates/pi-core-ffi/src/session_index.rs:55-60`) returns
  `ProjectRecord{display_path: String}` over FFI, and `AppModel.refreshProjects()`
  (`AppModel.swift:185-187`) already populates `AppModel.projects: [ProjectRecord]` on every
  `refreshSidebar()` call (startup, plus after every project/session action) — confirmed via grep
  that **no SwiftUI view reads `model.projects` today**.
- **The Slint app already has a working reference for this exact UX**: a project-switcher
  `ComboBox` (`crates/slinty-pi/ui/sidebar.slint:195-209`) fed by `Sidebar::refresh_projects`
  (`crates/pi-core/src/backend.rs:896-919`), which **filters out the current project**
  (`.filter(|p| Some(&p.display_path) != cwd_display.as_ref())`, line 904) and uses `display_path`
  directly as the switch target (`UiCmd::SwitchProject(PathBuf::from(display_path))`) — confirming
  `display_path` is already trusted, in production, as a real switchable path, not merely a
  display string.
- **The chat pane's title, not the sidebar's, is what the user is pointing at.**
  `ChatView.swift:80` — `.navigationTitle(model.activeSessionPath == nil ? "New Session" : "pi")`
  — this literal `"pi"` is what shows in the window's actual title bar (macOS's
  `NavigationSplitView` promotes the visible detail column's `navigationTitle` to the window
  titlebar). `SidebarView`'s own `.navigationTitle(projectDisplayName)` (`SidebarView.swift:38`,
  using `(model.currentProject as NSString).lastPathComponent`, defined at lines 101-103) only
  labels the sidebar column header, not the window title, and shows the folder name only — never
  the full path anywhere.
- `AppModel.currentProject: String` (`AppModel.swift:57`, `private(set)`) is a single value seeded
  from one `UserDefaults` key (`AppModel.swift:400-409`) — no history array exists client-side;
  the previous-projects list has to come from `SessionIndex`, not from anything Swift already
  tracks.
- `SwiftyPiApp.swift` (10 lines, full file) is a bare `WindowGroup { ContentView() }` — no
  `.commands{}`/`CommandGroup` exists anywhere in the Swift app yet; this is the app's first
  File-menu customization. `ContentView.swift:7` currently owns `AppModel` as its own
  `@State private var model = AppModel()` — a File-menu command needs to reach that same instance,
  so it has to move up a level to `SwiftyPiApp`.
- `SidebarView.swift`'s `pickProject()` (lines 105-113) is today's only project-switch entry
  point: opens an `NSOpenPanel` (directories only), then `Task { await model.switchProject(to:
  url.path) }`.

### Design

- **Chat pane title**: replace `ChatView.swift:80`'s hardcoded title with the project name, plus
  a `.navigationSubtitle` carrying the full path — `.navigationSubtitle` is the native
  macOS-only SwiftUI modifier for exactly this "primary title + secondary detail line in the title
  bar" pattern (always visible, no hover/tooltip needed). Centralize the "last path component"
  computation onto `AppModel` as `var projectDisplayName: String` (replacing the private computed
  property currently duplicated in `SidebarView`) so both views read the same thing:
  ```swift
  // ChatView.swift
  .navigationTitle(model.projectDisplayName)
  .navigationSubtitle(model.currentProject)
  ```
  This drops the old "New Session" vs "pi" title distinction — that state is already visible via
  the sidebar (the active row, and the synthesized "New session" placeholder row), so the title
  bar is better spent always showing real project identity instead.
- **File → Open Recent Project**: hoist `AppModel` from `ContentView` up to `SwiftyPiApp`
  (`@State private var model = AppModel()`), passing it down (`ContentView(model: model)`);
  `ContentView` changes its own `@State private var model = AppModel()` to a plain
  `let model: AppModel` parameter (update its `#Preview` to `ContentView(model: AppModel())`).
  This is the direct, single-window equivalent of what `@FocusedValue` solves for multi-window
  apps — unneeded complexity here, since this app has exactly one window.
- New `.commands { CommandGroup(after: .newItem) { ... } }` in `SwiftyPiApp.swift`:
  ```swift
  Menu("Open Recent Project") {
      let recents = model.projects.filter { $0.displayPath != model.currentProject }
      if recents.isEmpty {
          Text("No Previous Projects")
      } else {
          ForEach(recents, id: \.displayPath) { project in
              Button(project.displayPath) {
                  Task { await model.switchProject(to: project.displayPath) }
              }
          }
      }
  }
  Button("Open Project…") {
      Task { await model.promptSwitchProject() }
  }
  .keyboardShortcut("o")
  ```
  Mirrors Slint's own filter-out-current-project precedent exactly. Reuses `model.projects`'s
  existing pull-based refresh as-is (populated at startup and after every sidebar action) — no new
  refresh mechanism, consistent with how `model.sessions`/`model.models` are already kept "fresh
  enough" elsewhere in this app.
- **Shared picker**: move `SidebarView.pickProject()`'s `NSOpenPanel` logic onto `AppModel` as
  `func promptSwitchProject() async` (new `import AppKit` in `AppModel.swift`, matching
  `RowView.swift`'s existing precedent for a Swift file needing one AppKit type), so the File
  menu's "Open Project…" and the sidebar's existing "Switch Project" toolbar button call the same
  code instead of duplicating the panel setup. `SidebarView`'s toolbar button calls
  `model.promptSwitchProject()` directly; `pickProject()` is removed.
- **No backend changes** — `pi-sessions`/`pi-core-ffi` already expose everything this needs; this
  is Swift-only wiring of already-shipped data.
- **Explicit scope cut**: no recency ordering. `list_projects` sorts alphabetically by
  `display_path` and has no last-used timestamp anywhere in `Project`/`ProjectRecord` — matches
  Slint's own `ComboBox` exactly (same ordering), so this isn't a regression relative to the
  existing reference app. Adding recency would mean extending `pi_sessions::Project` with a
  timestamp derived from `session_dir`'s mtime or `SessionMeta::last_timestamp` of its newest
  session — a real, separable backend change, not bundled into this fix. Flagged so it isn't
  mistaken for an oversight; worth reconsidering only if an alphabetical list proves annoying with
  many projects in practice.

### Work breakdown

1. `AppModel.swift`: add `import AppKit`; add `var projectDisplayName: String` computed property;
   add `func promptSwitchProject() async` (ported from `SidebarView.pickProject()`).
2. `SidebarView.swift`: replace the private `projectDisplayName` computed property with calls to
   `model.projectDisplayName`; remove `pickProject()`, calling `model.promptSwitchProject()`
   directly from the toolbar button.
3. `ChatView.swift`: replace the `.navigationTitle` line with `model.projectDisplayName` plus a
   new `.navigationSubtitle(model.currentProject)`.
4. `ContentView.swift`: change `@State private var model = AppModel()` to `let model: AppModel`;
   update `#Preview` to pass one in.
5. `SwiftyPiApp.swift`: add `@State private var model = AppModel()`; pass to
   `ContentView(model: model)`; add `.commands { CommandGroup(after: .newItem) { ... } }` per
   Design.
6. Verification.

### Risks

- `display_path`'s "lossy for a session-less project" caveat (Verified facts) is a pre-existing,
  already-accepted characteristic of data this app already pulls over FFI — not a new risk
  introduced here, and unreachable in practice since `list_projects` only lists directories that
  already contain at least one session file.
- Dropping the "New Session"/"pi" distinction from the chat title (Design) is a deliberate call,
  not an oversight — flagged in case it's missed in practice; the same information remains visible
  via the sidebar's active-row/placeholder-row state.

### Acceptance criteria

- Clean `xcodebuild -project SwiftyPi.xcodeproj -scheme SwiftyPi build`.
- Manual: the chat pane's title bar shows the current project's folder name with the full path as
  a subtitle underneath, updating immediately after any project switch; the File menu has an
  "Open Recent Project" submenu listing every other known project (excluding the current one) that
  switches correctly when clicked, plus an "Open Project…" item that opens the same native folder
  picker the sidebar's toolbar button already does.

---

## Fix — Keyboard-driven extension-UI dialogs (permission prompts)

**Status: implemented and committed** (commit `5175861`) — `confirm` gets `y`/Allow and `n`/Deny;
`select` (what the real installed `pi-permission-system` extension actually uses) gets numbered
rows (1-9) plus a visible `(y)`/`(s)`/`(n)` hint wherever `mnemonic(for:)` matches, both driven by
that one shared function so the hint and the binding can't disagree; Cancel responds to Escape.
Clean `xcodebuild`, confirmed working by the user against the real extension.

**Goal:** asked directly — "I would like to update the permission dialog box so that it can work
without having to use the mouse. So maybe have 1, 2, 3... kb short cuts for each option, or y to
accept, s to accept for session, etc." The permission-gating dialogs (`ExtensionDialogView.swift`,
built in SW5) currently require clicking; this adds real keyboard shortcuts so the common case
(review a tool-call permission prompt, respond) never needs the mouse.

### Verified facts

- **The real, already-installed `pi-permission-system` extension uses `method: "select"`, not
  `"confirm"`** — reconfirmed live via `cargo run -p pi-core-ffi --example spike_check --
  dialogs` just now: a real gated bash command produced `extension_dialog: method=select id=...
  title=Some("Permission Required\nCurrent agent requested bash command '...' (matched '*').
  Allow this command?")`. This matches SW5's own original finding (the plan's aspirational
  `confirm` hypothesis was wrong even then) — so the shortcut design has to work for `select`
  (a dynamic list of option strings), not just the simpler `confirm` (fixed Allow/Deny) dialog.
- **A real "approve for the rest of the session" outcome already exists in the extension's own
  protocol**, confirmed via `~/.pi/agent/extensions/pi-permission-system/logs/pi-permission-
  system-permission-review.jsonl`: distinct logged resolutions include `"approved"`, `"denied"`,
  and **`"session_approved"`** (e.g. a real entry: `{"resolution":"session_approved", "command":
  "source ~/.cargo/env && cargo build...", ...}`). This confirms the underlying capability the
  user is describing ("s to accept for session") is real pi-extension behavior, not a feature this
  app would need to invent — it's presumably driven by the user picking a third option (beyond
  plain Allow/Deny) in the `select` dialog's `options` list.
- **The exact display text of that third option is not confirmed** — `spike_check`'s `PrintSink`
  logs `method`/`id`/`title`/`message` but not `options` (an oversight in that existing debug tool,
  not fixed here since it's not blocking), and no local copy of the extension's own source was
  found on this machine to read the literal strings it constructs. **This means shortcuts can't
  safely be hardcoded to specific option text.** Design below treats **position-based numbering
  (1-9) as the one reliable, protocol-agnostic mechanism** — it works correctly no matter what the
  options actually say — and treats letter mnemonics (`y`/`s`/`n`) as a best-effort layer on top,
  activated only when an option's text happens to obviously match (`"allow"`/`"yes"` → `y`,
  `"session"` → `s`, `"deny"`/`"no"` → `n`), never required.
- **`crates/pi-core-ffi/src/dialog.rs`** confirmed current (unchanged since SW5):
  `ExtensionDialogRecord{id, method, title, message, options: Option<Vec<String>>, placeholder,
  prefill, timeout}`, `ExtensionDialogReply::{Value{value}, Confirmed{confirmed}, Cancelled}`. No
  Rust/wire-protocol changes needed anywhere — this whole fix is Swift-only input handling on top
  of the exact same `AppModel.replyToCurrentDialog(_:)` calls the dialog views already make.
- **`macos/swifty-pi/SwiftyPi/ExtensionDialogView.swift`** (full file, 160 lines, unchanged since
  SW5) confirmed current:
  - `ExtensionDialogModifier.alertActions(for:)` (lines 62-80) — the `confirm`/`input` `.alert`'s
    action buttons. For `confirm`: `Button("Deny", role: .cancel) { ... }` / `Button("Allow") {
    ... }`, currently no explicit `.keyboardShortcut` on either (relies only on `.alert`'s implicit
    NSAlert-backed defaults — Escape for the `.cancel`-role button; Return for whichever button
    isn't `.cancel`, since NSAlert makes the first non-cancel button its default).
  - `SelectDialogView` (lines 88-117) — `ForEach(dialog.options ?? [], id: \.self) { option in
    Button(option) { model.replyToCurrentDialog(.value(value: option)) } }`, no numbering, no
    shortcuts at all today. Its Cancel toolbar button also has no explicit shortcut (Escape
    currently does nothing here, unlike the alert dialogs).
  - `EditorDialogView` (lines 119-153) — Cancel/Submit as plain toolbar buttons, no shortcuts.
    **Out of scope** for this fix (the user asked about "the permission dialog," which is
    `confirm`/`select`, not the free-text editor case) — left untouched.
  - `input`'s `TextField` inside the shared `.alert` already benefits from NSAlert's implicit
    Return-submits behavior via the same mechanism as `confirm`'s Allow button — also out of scope
    (not what was asked; not touched).

### Design

- **`confirm` dialogs** (`alertActions`, lines 72-79): add explicit shortcuts matching the user's
  own suggested mnemonics — `"y"` for Allow, `"n"` for Deny (Escape/`.cancel`-role stays as an
  equivalent second way to deny, unchanged):
  ```swift
  Button("Deny", role: .cancel) {
      model.replyToCurrentDialog(.confirmed(confirmed: false))
  }
  .keyboardShortcut("n", modifiers: [])
  Button("Allow") {
      model.replyToCurrentDialog(.confirmed(confirmed: true))
  }
  .keyboardShortcut("y", modifiers: [])
  ```
- **`select` dialogs** (`SelectDialogView`) — the one that actually matters most, since it's what
  the real installed extension uses. **Per direct follow-up, the mnemonic letter is shown inline,
  not hidden** — a single `mnemonic(for:)` function is the one source of truth for both the
  visible hint text and the actual key binding, so the two can never drift out of sync (the
  earlier draft of this plan computed the hidden-button match separately from a display label,
  which would have risked exactly that):
  ```swift
  /// The one place option-text heuristics live — drives both the visible
  /// "(y)"/"(s)"/"(n)" hint and the actual hidden-button key binding below,
  /// so they can never disagree. Checked in this order so a hypothetical
  /// "Allow for session" option resolves to `s`, not `y`.
  private func mnemonic(for option: String) -> Character? {
      let lower = option.lowercased()
      if lower.contains("session") { return "s" }
      if lower.contains("allow") || lower.contains("yes") { return "y" }
      if lower.contains("deny") || lower.contains("no") { return "n" }
      return nil
  }
  ```
  Each option row gets a leading number (1-9, matching its position, with its own bare-digit
  `.keyboardShortcut` — reliable regardless of option text, directly answers "1, 2, 3...
  shortcuts for each option") **and**, when `mnemonic(for:)` matches, a trailing `"(y)"`/`"(s)"`/
  `"(n)"` hint so the letter shortcut is fully discoverable, not just the number:
  ```swift
  ForEach(Array(options.enumerated()), id: \.offset) { index, option in
      Button {
          model.replyToCurrentDialog(.value(value: option))
      } label: {
          HStack {
              if index < 9 {
                  Text("\(index + 1)")
                      .font(.caption.monospacedDigit())
                      .foregroundStyle(.secondary)
                      .frame(width: 16, alignment: .trailing)
              }
              Text(option)
              if let letter = mnemonic(for: option) {
                  Text("(\(letter))")
                      .font(.caption)
                      .foregroundStyle(.secondary)
              }
              Spacer(minLength: 0)
          }
      }
      .keyboardShortcut(index < 9 ? KeyEquivalent(Character("\(index + 1)")) : nil, modifiers: [])
  }
  ```
  (`.keyboardShortcut` accepts an optional `KeyEquivalent` in this initializer form — `nil` for
  the 10th-and-beyond option just skips the modifier, an accepted edge case since real permission
  prompts never have anywhere near 9 options.)
  - **The letter itself is still wired via small hidden buttons** — not attached to the same
    `Button` as the number (a single SwiftUI `Button` only honors one `.keyboardShortcut`), the
    standard SwiftUI idiom for an extra keyboard-only affordance, but now driven by the same
    `mnemonic(for:)` function the visible hint uses, guaranteeing the label always matches the
    actual binding:
    ```swift
    .background {
        ForEach(["y", "s", "n"] as [Character], id: \.self) { letter in
            if let match = options.first(where: { mnemonic(for: $0) == letter }) {
                Button("") { model.replyToCurrentDialog(.value(value: match)) }
                    .keyboardShortcut(KeyEquivalent(letter), modifiers: [])
                    .hidden()
            }
        }
    }
    ```
    If no option matches a given letter, it's simply never bound and never shown — no crash, no
    wrong guess forced onto an unrelated option.
  - Cancel gets an explicit shortcut too, since (unlike the alert dialogs) nothing currently backs
    Escape here: `Button("Cancel") { ... }.keyboardShortcut(.cancelAction)`.

### Work breakdown

1. `ExtensionDialogView.swift`: `confirm`'s `alertActions` gains `y`/`n` shortcuts on Allow/Deny.
2. `SelectDialogView`: `mnemonic(for:)` helper; numbered rows (1-9) with per-row
   `.keyboardShortcut` and a visible `"(y)"/"(s)"/"(n)"` hint wherever `mnemonic(for:)` matches;
   hidden buttons wiring the actual `y`/`s`/`n` key bindings off the same helper; Cancel gets
   `.keyboardShortcut(.cancelAction)`.
3. Verification (below) — both via the demo/synthetic path if feasible and, since this is exactly
   the kind of thing that needs a real human at the keyboard, a manual pass by the user against
   the real `pi-permission-system` extension.

### Risks

- **NSAlert-backed `.alert` button shortcuts**: SwiftUI bridges `.keyboardShortcut` on `.alert`
  action buttons to `NSAlert`'s `keyEquivalent` on macOS, which is expected to work, but this
  exact combination (bare single-letter key on an alert button) hasn't been exercised anywhere
  else in this codebase yet — confirm with a real keypress during verification, not just a clean
  build.
  - **Mnemonic letters are a best-effort heuristic, not a protocol guarantee** — flagged
  extensively above; the numbered shortcuts are the actual, always-correct answer to the user's
  request, and remain fully functional even if every heuristic match is wrong or absent for a
  given extension's option wording.
- A `select` dialog with more than 9 options gets no number shortcut past 9 — realistic permission
  prompts are 2-3 options, so this is a non-issue in practice, not a gap worth solving for.

### Acceptance criteria

- Clean `xcodebuild -project SwiftyPi.xcodeproj -scheme SwiftyPi build`. Manual, against the real
  installed `pi-permission-system` extension: trigger a gated bash command, confirm each option in
  the resulting dialog shows a number and pressing that digit answers it exactly as clicking would;
  confirm any option showing a `"(y)"/"(s)"/"(n)"` hint responds correctly to that key, and confirm
  no hint (and no misfire) appears for options that don't obviously match any heuristic; confirm
  Escape cancels the select dialog; confirm a plain `confirm`-method dialog (if one can still be
  triggered/simulated) responds correctly to `y`/`n`.

---

## Fix — Composer auto-focus + prompt history (Up/Down arrows)

**Status: implemented and committed** (commit `ad7ada3`) — `@FocusState` on the composer
`TextField`, set in `.onAppear`; Up/Down history recall with the approximated
empty-field-or-already-navigating trigger (per direct follow-up, chosen over a custom
AppKit-backed text view for true cursor-position detection). Clean `xcodebuild`; a real-run
verification pass is pending confirmation from the user.

**Goal:** asked directly — "can the initial focus when starting the app be in the input field?
Also, I would like the up and down arrows to be able navigate through the history of prompts: if
the cursor is at the beginning and user presses up, show previous prompt (save current unsubmitted
prompt); if the cursor is at the end and user presses down, go to the next prompt if any." Two
composer UX gaps: no keyboard focus on launch (every session starts requiring a click), and no way
to recall previously-sent prompts.

### Verified facts

- **Auto-focus has a direct Slint precedent to port.** `crates/slinty-pi/ui/app.slint:643-645`
  exposes `public function focus-input() { input.focus(); }`, called from
  `crates/slinty-pi/src/main.rs:411` immediately before `app.run()?;` — and reused again at
  `app.slint:604` to restore focus after the command palette closes. The Swift app has **no
  `@FocusState` usage anywhere** (`grep -rn "FocusState" macos/swifty-pi/SwiftyPi/` — zero hits)
  — this is new plumbing for Swift, but a direct, simple port of proven behavior.
- **Prompt history has no Slint equivalent at all** — confirmed by exhaustive grep across
  `app.slint`/`main.rs`/`backend.rs` for `history`-in-a-composer-recall sense and for
  `Key.Up`/`Key.Down`/`UpArrow`/`DownArrow`: zero matches. The composer's only `key-pressed`
  handler in Slint (`app.slint:895-901`) handles Return/Shift+Return only; `submit()`
  (`app.slint:661-666`) sends and clears the input without storing the sent text anywhere. This is
  genuinely new UX for this fix to design, not a port.
- **SwiftUI's plain `TextField` on macOS 14 has no cursor-position access** — confirmed no way to
  read selection/cursor location from the composer's current `TextField(text:axis:)` without
  replacing it with a custom `NSViewRepresentable`-backed text view. Per direct follow-up, the
  chosen design is the **approximated** trigger condition (empty field, or already mid-recall) —
  not real cursor-position detection — keeping the composer as a plain `TextField`, no new AppKit
  bridging component. `.onKeyPress(_:action:)` (SwiftUI 5 / macOS 14+, matching this app's
  deployment target exactly) is the mechanism: it intercepts a key press on the focused view and
  returns `.handled` (consume) or `.ignored` (pass through to normal `TextField` behavior, e.g.
  real cursor movement within multi-line text).
- **`macos/swifty-pi/SwiftyPi/ChatView.swift`** (354 lines) confirmed current — composer `HStack`
  at lines 66-102 (paperclip attach button, then `TextField("Message pi…", text: $draft, axis:
  .vertical)` at line 74, then Send/Abort); `send()` at lines 283-288 (trims `draft`, guards
  non-empty, calls `model.send(prompt)`, resets `draft = ""` — the only place `draft` is reset
  today). `draft: String` is `@State private var draft` local to `ChatView` (confirmed still true,
  matches SW9/SW10's own findings) — the natural place for history state too, since this is purely
  a composer-input UI concern, not core session state `AppModel` needs to know about.

### Design

- **Auto-focus**: new `@FocusState private var composerFocused: Bool` on `ChatView`, bound via
  `.focused($composerFocused)` on the composer `TextField`, set `true` in a `.onAppear` on the
  `TextField` itself (mirrors `focus-input()`'s "call it once, right when the composer exists"
  timing without needing to thread anything through `AppModel`/`ContentView`).
- **Prompt history** — new `@State` alongside `draft` in `ChatView`:
  ```swift
  @State private var promptHistory: [String] = []
  @State private var historyOffset = 0   // 0 = not navigating; 1 = most recent; up to promptHistory.count
  @State private var savedDraft = ""
  @State private var isProgrammaticDraftChange = false
  ```
  Scoped to the view's lifetime (in-memory only, resets on app relaunch, persists across
  project/session switches within the same launch) — the simplest reasonable default; not
  persisted to disk, not scoped per-session, since none of that was asked for and it's easy to
  narrow later if it turns out to matter.
  - `send()` appends the sent prompt and resets navigation state:
    ```swift
    private func send() {
        let prompt = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !prompt.isEmpty else { return }
        model.send(prompt)
        promptHistory.append(prompt)
        historyOffset = 0
        draft = ""
    }
    ```
  - Two `.onKeyPress` modifiers on the composer `TextField`:
    ```swift
    .onKeyPress(.upArrow) {
        guard draft.isEmpty || historyOffset > 0 else { return .ignored }
        guard historyOffset < promptHistory.count else { return .handled }
        if historyOffset == 0 { savedDraft = draft }
        historyOffset += 1
        setDraft(promptHistory[promptHistory.count - historyOffset])
        return .handled
    }
    .onKeyPress(.downArrow) {
        guard historyOffset > 0 else { return .ignored }
        historyOffset -= 1
        setDraft(historyOffset == 0 ? savedDraft : promptHistory[promptHistory.count - historyOffset])
        return .handled
    }
    ```
    The `draft.isEmpty || historyOffset > 0` guard is the approximated stand-in for "cursor at the
    beginning" — Up only triggers recall when there's nothing to move a cursor through (empty) or
    we're already navigating; any other Up press (real text present, not navigating) is `.ignored`
    and falls through to the `TextField`'s normal cursor-up behavior. Down only ever triggers while
    already navigating (`historyOffset > 0`), matching "only recall forward once you've recalled
    backward" — a plain Down in normal typing is always `.ignored`.
  - `setDraft(_:)` funnels every programmatic write through one place and flags it, so a new
    `.onChange(of: draft)` can tell "we set this" apart from "the user typed," canceling history
    navigation only on a real edit:
    ```swift
    private func setDraft(_ text: String) {
        isProgrammaticDraftChange = true
        draft = text
    }
    ```
    ```swift
    .onChange(of: draft) {
        if isProgrammaticDraftChange {
            isProgrammaticDraftChange = false
        } else {
            historyOffset = 0
        }
    }
    ```
    The existing `.onChange(of: model.pendingComposerAppend)`/`.onChange(of:
    model.pendingComposerReplace)` handlers (SW9/SW10) already assign `draft` directly, not
    through `setDraft` — that's correct as-is: an attach-triggered `@path` append or a fork-prefill
    replace are real content changes, so letting them fall through to the new `.onChange(of:
    draft)`'s "cancel history navigation" branch is exactly the right behavior, not a conflict to
    special-case.

### Work breakdown

1. `ChatView.swift`: `@FocusState` + `.focused($composerFocused)` + `.onAppear { composerFocused =
   true }` on the composer `TextField`.
2. `ChatView.swift`: `promptHistory`/`historyOffset`/`savedDraft`/`isProgrammaticDraftChange`
   state; `send()` gains the history-append + offset-reset; `setDraft(_:)` helper; the two
   `.onKeyPress` modifiers; the new `.onChange(of: draft)` history-cancel handler.
3. Verification (below).

### Risks

- The empty-field-or-already-navigating trigger is an approximation, not true cursor-position
  detection — flagged directly to the user during planning as the trade-off for staying with a
  plain `TextField` instead of a custom AppKit-backed text view; accepted per their explicit
  choice. A cursor manually placed at position 0 within otherwise-present text won't trigger
  recall (falls through to normal cursor-up movement instead) — an accepted, known gap, not an
  oversight.
- `.onKeyPress` is a comparatively new SwiftUI API (introduced alongside this app's exact
  deployment target) — not yet exercised anywhere else in this codebase; confirm with a real
  keypress during verification, not just a clean build, per this project's own established
  lesson about typecheck/build not fully proving runtime behavior.

### Acceptance criteria

- Clean `xcodebuild -project SwiftyPi.xcodeproj -scheme SwiftyPi build`. Manual: launching the app
  (or switching to a session) puts keyboard focus directly in the composer with no click needed;
  sending a few prompts, then pressing Up from an empty composer recalls them most-recent-first,
  Up again goes further back, Down goes forward and eventually restores whatever was typed (if
  anything) before Up was first pressed; typing after a recall cancels navigation (a subsequent Up
  starts a fresh recall from the most recent prompt again, not mid-list); Up/Down do nothing
  unexpected while actively typing a non-empty, non-recalled draft (normal cursor movement, if any,
  is unaffected).

---

## Named-only follow-on scope (not planned in detail)

- **Extension-UI fire-and-forget surface** (`notify`/`setStatus`/`setWidget`/`setTitle`/
  `set_editor_text`) — explicitly scope-cut from SW5 in favor of just the four blocking dialog
  kinds. **Parked, not just deferred**, after research (below) surfaced a real blocker rather than
  just "not yet planned":
  - `notify {message, notifyType}`, `setStatus {statusKey, statusText}`, `setWidget {widgetKey,
    widgetLines, widgetPlacement}` have a *documented* shape (`docs/plans/M4-agent-trust.md:12-14`)
    — though unverified against this repo's current on-disk state; an earlier live capture during
    SW5 planning saw real `setStatus` traffic from the installed `pi-permission-system` extension,
    but that was an ephemeral observation, not something re-derivable from disk now.
  - `setTitle`/`set_editor_text`'s payload field names are **undocumented anywhere** — not in this
    repo, not citable from pi's own (not-vendored-here) `rpc.md`. Implementing them means guessing
    field names (e.g. `{title}`/`{text}`), which the user chose not to do speculatively.
  - `pi-core`'s own Slint reference has no working implementation for any of the five either (M4,
    which would own this, has status "planned" — unstarted); `notify` is the sole partial
    exception, already printing `req.message` as an info row (`crates/pi-core/src/backend.rs:
    2732-2753`).
  - Revisit once either upstream documents `setTitle`/`set_editor_text`'s shape, or a real
    extension observed on this machine exercises them (giving the same kind of ground truth SW5's
    `pi-permission-system` capture and SW6/SW7's `spike_check.rs` real-`pi` runs provided).

## Verification

- SW0: `cargo fmt --check && cargo clippy --workspace && cargo test --workspace`, plus manual
  `SLINTY_DEMO=1 cargo run -p slinty-pi` and a spot-check of the `SLINTY_*_AFTER` hooks listed
  above — confirms zero behavior change from the extraction.
- SW1: run `apps/pi-mac/scripts/build-rust.sh` then build/run `PiMac.xcodeproj` in Xcode; send a
  real prompt against an installed `pi` and confirm streaming + abort both work; optionally drive
  `pi_core::demo_backend` through the Swift app for a display-less perf check.
- SW2: run the updated `apps/pi-mac/scripts/build-rust.sh` + `xcodebuild -scheme PiMac build`;
  exercise switch-project/new/delete/rename against a real `pi` install and confirm the sidebar and
  transcript stay in sync; `cargo test -p pi-sessions -p pi-core -p pi-core-ffi` green.
- SW3: `cargo test -p pi-render -p pi-core -p pi-core-ffi` green (32 moved tests unchanged); extend
  `examples/spike_check.rs` with a `history` mode exercising `switch_session`; run the updated
  `apps/pi-mac/scripts/build-rust.sh` + `xcodebuild -scheme PiMac build`; against a real `pi`
  install, confirm clicking an older sidebar session renders its actual history richly, a fresh
  prompt with a code block/table renders correctly once the turn settles, and relaunching the app
  restores the last-active session automatically.
- SW4: `cargo test -p pi-local -p pi-core -p pi-core-ffi` green (47 moved `local::*` tests plus
  relocated panel-formatter tests unchanged); extend `examples/spike_check.rs` with a `models` mode
  (live Hugging Face search at minimum; rapid-mlx/router/Ollama actions tolerant of not being
  installed); run the updated `apps/pi-mac/scripts/build-rust.sh` + `xcodebuild -scheme PiMac
  build`; open the Swift Models panel and confirm rapid-mlx/router/Ollama/auth sections render
  correctly for whatever's actually installed, HF search returns real results, and saving an API
  key round-trips through `~/.pi/agent/auth.json`.
- App diagnostics fix: `cargo fmt --check && cargo clippy --workspace && cargo test --workspace`;
  clean `xcodebuild -scheme PiMac build`; manually break `pi`'s discoverability and confirm the
  failure shows up in `log stream --predicate 'subsystem == "dev.slinty-pi.pi-mac"'`/Console.app,
  not just the pre-existing status-bar caption.
- Live-streaming rendering fix: `cargo test --workspace` green (new `preview_rows` unit tests);
  clean `xcodebuild -scheme PiMac build`; send a real prompt whose reply has a heading/code block/
  bold text and confirm it renders richly while still streaming, not just after the turn settles.
- SW5: `cargo test --workspace` green (new `dialog.rs` conversion + `apply()` routing tests); clean
  `xcodebuild -scheme PiMac build`; against the real, already-installed `pi-permission-system`
  extension, trigger a bash command outside its allow-list and confirm a native dialog appears
  showing its real gating message, Allow/Deny is reflected in both pi's behavior and the
  extension's own review log, and aborting mid-dialog doesn't hang the app.
- SW6: `cargo test --workspace` green (new `models.rs` label-formatting + `classify_rapid_mlx_dot`
  truth-table tests); clean `xcodebuild -scheme PiMac build`; against a real `pi`, confirm the
  composer picker lists actual configured models with correct labels and switching one sticks;
  wherever rapid-mlx is running locally, confirm the server dot's color matches a manual
  `curl <base>/health` and updates within ~5s (or immediately after a model switch).
- SW7: `cargo test --workspace` green (new `live_preview.rs` unit tests); clean `xcodebuild -scheme
  PiMac build`; against a real `pi`, send a prompt that triggers a real tool call and confirm the
  tool row appears/updates live while streaming (not only after settle), confirm a live thinking
  row too if the active model/thinking level emits reasoning content, and confirm a clean handoff
  to the finalized rows at turn-end with no stuck/duplicated live rows.
- User bubble + delete confirmation fix: clean `xcodebuild`; manual — user messages render
  right-aligned with a tinted background, and deleting a session prompts for confirmation first.
- Density control fix: clean `xcodebuild`; manual — cycling Verbose/Normal/Summary changes row
  visibility and tool-call expansion correctly, and the setting survives an app relaunch.
- SW8: `cargo test --workspace` green (new `thinking.rs`/stats tests); clean `xcodebuild`; against
  a real `pi`, confirm the thinking-level menu appears only for multi-level models and sticks when
  changed, and the usage ring + stats caption update after a real turn settles and after switching
  sessions.
- SW9: `cargo test --workspace` green; clean `xcodebuild`; against a real `pi`, confirm an attached
  image is reflected in the next reply, a non-image attachment appends `@path` text, Finder
  drag-and-drop works, and removing a chip before sending drops that attachment.
- SW10: `cargo test --workspace` green; clean `xcodebuild`; against a real `pi` session with a
  prior fork/branch, confirm the tree sheet shows the correct structure with the active node
  marked, and forking from an older message rewinds history and pre-fills the composer correctly.
- Markdown export fix: `cargo test --workspace` green (new `pi-render` tests); clean `xcodebuild`;
  exporting a real multi-turn session (with a code block and a tool call) produces a readable,
  valid markdown file.
- Project-visibility fix: clean `xcodebuild`; manual — the chat pane's title bar shows the current
  project name/path and updates on switch; the File menu's "Open Recent Project" lists known
  projects (excluding the current one) and switches correctly, and "Open Project…" opens the same
  picker as the sidebar's toolbar button.

---

## PR-splitting strategy for upstream review

**Context:** asked directly — the `swiftui` branch has diverged too far from `main` for one
reviewable PR. Verified via `git log --oneline main..swifty-pi` / `git diff --stat main...swifty-pi`:
**58 commits, 72 files changed, +16,394/-4,007 lines** against `main` (merge-base `ac50f15`).

**The key fact that makes this easy, not hard**: `git log --oneline main..swifty-pi` shows the
branch's commit history is **already perfectly ordered by milestone with zero interleaving** —
every SW-numbered milestone (and every fix) occupies a single contiguous run of commits, in the
same order this whole session built and tested them. This is a direct product of this session's
own "one feature = one or two commits, verified before moving on" discipline. It means **the split
needs zero rebasing, zero cherry-picking, zero history rewriting** — just picking existing commit
SHAs as branch cut points and opening a *stack* of PRs, each based on the previous one. No
destructive git operation is required anywhere in this strategy.

### The floor: Rust core vs. macOS app (what was explicitly asked for)

The very first 4 commits on the branch (`dc0f0a7`..`d2c20fd`) are exactly "isolate the shared Rust
code" — the SW0 milestone: scaffold `crates/pi-core`, move every Slint-free module into it behind
a `UiSink` trait, add live-streaming-path test coverage. **Zero new Swift/macOS files appear until
the very next commit** (`bedecc1`, which adds `crates/pi-core-ffi`). So the floor the user asked
for is already a clean, pre-existing boundary — no work needed to find it, just to cut there:

- **PR 1 — "Extract crates/pi-core behind a UiSink trait"** (`dc0f0a7^..d2c20fd`, 4 commits).
  Pure Rust, zero behavior change for the existing Slint app (that's the SW0 milestone's own
  acceptance bar, already met and tested). `git diff --shortstat` shows `+4670/-3966`, but that's
  almost entirely `git mv`-tracked file moves — GitHub's PR view collapses renamed files
  automatically, so the *actual* reviewable diff will look far smaller than the raw stat suggests.
  This PR is valuable independent of the SwiftUI work at all — it's a real refactor the Slint app
  benefits from on its own.
- **PR 2 — everything else.** Still ~15,700 lines. This alone doesn't really solve the problem —
  it's the mandatory floor, not a sufficient split by itself.

### Recommended split — decided (13 PRs, hosted in `fork`, one final rollup to `origin`)

**Confirmed (not just flagged): no push/branch-create access to `origin` (`tilladam/slinty-pi`).**
GitHub requires a PR's *base* branch to exist in the same repository the PR is opened against, so
the 12 intermediate stacking branches (everything except a final rollup) cannot live in `origin`
without that access. Revised strategy: **the entire 13-PR stack is hosted inside the user's own
fork** (`mkrus/slinty-pi`) — both base and head branches for every row live there, needing no
access beyond what already exists. Only **one final PR**, from the fork's fully-merged integration
branch to `origin/main`, ever needs to reach `origin` — a plain fork→upstream contribution PR
(head only, never base), which needs no special access on `origin` at all. Reviewers who want to
review the 13-row breakdown individually, rather than just the final rollup, need read/comment
access to the fork's PRs — a lighter ask than push access to `origin`.

Smaller/adjacent rows are still merged down from the initial 17-row draft to reduce
stack-management overhead, and every milestone is still its own contiguous commit range (confirmed
via `git log --oneline` across the full branch) — so this remains pure branch-cutting, no
rebasing. Only the destination repo changed, from `origin` to `fork`, for rows 1-13:

| # | PR | Commit range | ~Lines | Notes |
|---|---|---|---|---|
| 1 | Extract `pi-core` behind `UiSink` | `dc0f0a7^..d2c20fd` | +4670/-3966 (mostly moves) | SW0, zero behavior change |
| 2 | `pi-core-ffi` + minimal SwiftUI spike | `bedecc1..f50bb19` | ~1660 | SW1 — first Swift/macOS files appear here |
| 3 | Session sidebar | `4a3ffce..524d662` | ~1150 | SW2 |
| 4 | Extract `pi-render` + rich history rendering | `3e94552..cfd3a17` | ~1425 | SW3 |
| 5 | Extract `pi-local` + Models panel | `f823b8a..017875d` | ~1655 | SW4 |
| 6 | Diagnostics + live-streaming fix + Extension-UI dialogs | `ed5bd3d..477f3d3` | ~725 | small diagnostics/rendering fixes folded into the adjacent SW5 dialogs work |
| 7 | Composer model picker + server dot | `b850202..df779b0` | ~585 | SW6 |
| 8 | Live thinking/tool visibility + row polish | `93a7152..09f8fc8` | ~715 | SW7 + copy/delete-confirm/user-bubble |
| 9 | Rename `apps/pi-mac` → `macos/swifty-pi`, `PiMac` → `SwiftyPi` | `415dbc7` alone | ~50/-49 across 15 files | kept isolated on purpose — the one row not merged, so the mechanical rename never tangles with a real feature diff |
| 10 | UI polish batch: relocation, app icon, project visibility, density control, streaming indicator | `759a18d..9df5021` | ~830 | merges the four smallest post-rename polish commits into one |
| 11 | Composer completion: thinking-level picker + attach files/images | `8784840..a70e0bc` | ~830 | SW8 + SW9 merged — both are composer-surface features |
| 12 | Session branch tree + fork-from | `bbcff65` alone | ~605 | SW10 — kept separate from #11, a distinct UI surface (a new sheet, not the composer) |
| 13 | Keyboard-driven permission dialogs + composer focus/history | `5175861..dd88825` | ~340 code + ~3300 docs | includes both `docs/plans` snapshot commits (`184366a`, `dd88825`) — reviewers can skip that file, it's a plan-doc mirror, not code |

### Mechanics — a stack inside `fork`, then one rollup PR to `origin`

Each row becomes a real branch cut directly off the existing commit history, no rebasing —
pushed to `fork` (`mkrus/slinty-pi`), not `origin`:

```sh
git branch swiftui-01-core-extraction d2c20fd
git branch swiftui-02-ffi-spike f50bb19
git branch swiftui-03-sidebar 524d662
# ...one per row, each named branch pointing at that row's last commit...
git push fork swiftui-01-core-extraction swiftui-02-ffi-spike swiftui-03-sidebar ...
```

Then open each PR **within `fork` itself**, base set to the previous row's branch (not `main`):
```sh
gh pr create --repo mkrus/slinty-pi --base swiftui-01-core-extraction --head swiftui-02-ffi-spike --title "..."
```
except PR 1, whose base is `fork`'s own copy of `main`. This is the standard "stacked PR" pattern:
each PR's diff only ever shows *that row's* commits, since everything from earlier rows is already
common history with its base branch. As each PR merges (bottom-up) *within `fork`*, retarget the
next one's base to `fork`'s `main` — a one-click "change base branch" on GitHub, no local git
changes needed. **No commit is ever rewritten, no force-push is ever required** — every branch
here is a plain, non-destructive pointer into history that already exists.

Once the last row merges into `fork`'s `main` (or a dedicated integration branch, if `fork`'s
`main` should stay a clean mirror of `origin/main` — worth deciding at that point, not now), open
**one final PR**: `fork`'s integration branch → `origin/main`:
```sh
gh pr create --repo tilladam/slinty-pi --base main --head mkrus:swiftui-integration --title "..."
```
This is the only step that touches `origin`, and it's the ordinary "propose changes from a fork"
flow every GitHub contributor without write access already uses — no elevated permissions needed.

**Reviewer access — confirmed workable**: per direct follow-up, the intended reviewers are
teammates who can be added as collaborators on `mkrus/slinty-pi`. So the 13-way breakdown does
deliver its full benefit — grant them access to the fork before opening PRs 1-13 there, so they
can review the milestone-sized diffs directly, same experience as if the stack lived in `origin`.
Only the final rollup PR (fork's integration branch → `origin/main`) needs to exist in `origin`
itself.

### Status: the 13-PR stack is created and open in `mkrus/slinty-pi`

All 13 branches were cut from the existing commit SHAs above, pushed to `fork`, and opened as
stacked PRs (each base = the previous row's branch, PR 1's base = `fork`'s `main`, which was
confirmed identical to `origin/main` at cut time):

1. https://github.com/mkrus/slinty-pi/pull/1 — Extract `pi-core` behind a `UiSink` trait
2. https://github.com/mkrus/slinty-pi/pull/2 — `pi-core-ffi` + minimal SwiftUI chat spike
3. https://github.com/mkrus/slinty-pi/pull/3 — Session sidebar in Swift
4. https://github.com/mkrus/slinty-pi/pull/4 — Extract `pi-render` + rich history rendering, restore/switch sessions
5. https://github.com/mkrus/slinty-pi/pull/5 — Extract `pi-local` + local-model panel in Swift
6. https://github.com/mkrus/slinty-pi/pull/6 — App diagnostics, live-streaming rendering fix, extension-UI dialogs
7. https://github.com/mkrus/slinty-pi/pull/7 — Composer model picker + server-dot health indicator
8. https://github.com/mkrus/slinty-pi/pull/8 — Live thinking/tool-call visibility + row polish
9. https://github.com/mkrus/slinty-pi/pull/9 — Rename `apps/pi-mac` → `macos/swifty-pi`, `PiMac` → `SwiftyPi`
10. https://github.com/mkrus/slinty-pi/pull/10 — UI polish batch: app icon, project visibility, density control, streaming indicator
11. https://github.com/mkrus/slinty-pi/pull/11 — Composer completion: thinking-level control + attach files/images
12. https://github.com/mkrus/slinty-pi/pull/12 — Session branch tree + fork-from
13. https://github.com/mkrus/slinty-pi/pull/13 — Keyboard-driven permission dialogs + composer focus/history

**Not yet done**: inviting teammate reviewers as collaborators on `mkrus/slinty-pi` (need names/
GitHub handles from the user), and — once the stack is reviewed and merged down within the fork —
opening the single final rollup PR from the fork's integration branch to `origin/main`.
