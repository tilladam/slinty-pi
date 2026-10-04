# bummer.md — what did not work here and why.
# Append-only during tasks; the `bummer` skill merges, promotes and retires.
# Template per entry:
#
# ## YYYY-MM-DD · title · #tags/paths
# Situation: task, constraints, versions
# Tried: what seemed reasonable / what was proposed
# Outcome: what failed or was insufficient (observed, not inferred)
# Evidence: dated observation or user correction, reference if any
# Hypothesis: suspected cause (optional; keep separate from Outcome)
# Next time: verified alternative, or suggestion — say which
# Revisit when: conditions that would invalidate this
# Cost: what the dead end cost (time, reverts, user corrections)
# Scope: project | global-candidate     Status: active
# recurred: YYYY-MM-DD note   (append to the existing entry, no new entry)

# Seeded 2026-10-04 with Slint gotchas: slinty-pi's own (carried over from the project memory
# "slint-gotchas-slinty-pi", dated by when they were recorded there), the Slint-related entries
# of yapper's bummer.md (summarised; the original entry is the reference), and one second-hand
# Flectar Mail lesson. Entries marked global-candidate apply to Slint work in general and are
# meant to move into the shared crates repo's docs (docs/plans/shared-crates.md, phase 1).

## 2026-08-04 · width bindings inside a Flickable loop · #slint #layout slint/slinty-pi/ui/app.slint
Situation: transcript bubbles in Slint 1.17; capping bubble width relative to the window.
Tried: binding a child's `width`/`max-width` to an ancestor's width inside a `Flickable`.
Outcome: layoutinfo binding loop (compile warning, runtime panic risk).
Evidence: recorded in project memory by 2026-08-04; workaround in app.slint since.
Next time: use `ListView` for width-dependent content (it fixes delegate width itself); cap widths with stretch-spacer Rectangles (`horizontal-stretch: 1; min-width: Npx`) beside the content. Verified.
Revisit when: Slint's Flickable resolves cross-axis width without a loop.
Cost: not recorded.
Scope: global-candidate     Status: active

## 2026-08-04 · TouchArea sized from its child loops · #slint #layout slint/slinty-pi/ui
Situation: clickable chips in Slint 1.17.
Tried: `TouchArea { width: chip.width; }` around a chip Rectangle.
Outcome: binding loop: the child Rectangle defaults to filling its parent.
Evidence: recorded in project memory by 2026-08-04.
Next time: put the TouchArea inside the sized element. Verified.
Revisit when: never likely; it follows from fill-parent defaults.
Cost: not recorded.
Scope: global-candidate     Status: active

## 2026-08-04 · trailing hover TouchArea swallowed nested button clicks · #slint #input slint/slinty-pi/ui/models.slint
Situation: rows in the models panel with a row-hover background and nested serve/load/unload buttons.
Tried: `touch := TouchArea {}` declared after the row's HorizontalLayout, used only for `has-hover`.
Outcome: the nested buttons were dead: an MCP `click_element` produced no state change, while the same backend command via `SLINTY_SERVE_RAPID_MLX_AFTER` worked.
Evidence: 2026-08-04, `CachedModelItem`/`RouterModelItem`.
Hypothesis: siblings hit-test in declaration order, last declared on top; a TouchArea consumes the press even without a `clicked` handler.
Next time: declare the hover-only TouchArea first, give it geometry that excludes the buttons, or keep hover state in the button TouchAreas. Verified (first option).
Revisit when: Slint forwards unhandled presses to lower siblings.
Cost: buttons shipped broken until found; test hooks that bypass the UI hid it.
Scope: global-candidate     Status: active

## 2026-08-04 · ComboBox current-index -1 shows model[0] as selected · #slint #widgets slint/slinty-pi/ui/sidebar.slint
Situation: project switcher listing the other projects, current one filtered out.
Tried: `current-index: -1` from Rust to mean "nothing selected".
Outcome: `ComboBoxBase.reset-current()` clamps to 0 on every model change; the box displayed model[0] as chosen, and clicking it switched the app into an unrelated project.
Evidence: Slint's `internal/compiler/widgets/common/combobox-base.slint`; real bug, recorded 2026-08-04.
Next time: prepend a real placeholder entry ("Switch project…") and point `current-index: 0` at it. Verified.
Revisit when: std-widgets ComboBox gains an explicit empty state.
Cost: one user-visible wrong-project switch.
Scope: global-candidate     Status: active

## 2026-08-03 · animation-tick() bindings kept the GPU renderer busy · #slint #perf slint/slinty-pi/ui/app.slint
Situation: thinking ellipsis and sidebar live-dot animated with `animation-tick()`.
Tried: leaving those bindings live while a turn streams but is quiet (e.g. waiting on a tool).
Outcome: about 60% of a core in a debug build while nothing visible changed.
Evidence: macOS `sample <pid> 5` showing the DisplayLink→draw branch on the main thread; fixed in 2ee12e5.
Hypothesis: `animation-tick()` dirties the window every vsync and the GPU renderers repaint the whole scene (partial repaint is software-renderer-only).
Next time: step discrete states with a `Timer`; for pulses, Timer-toggle a property and `animate` it. Verified.
Revisit when: GPU renderers get partial repaint.
Cost: battery and CPU until diagnosed.
Scope: global-candidate     Status: active

## 2026-08-04 · box layouts size rows before wrapped Text height-for-width · #slint #layout slint/slinty-pi/ui/app.slint
Situation: markdown tables (`TableBlock`) with long wrapped cells.
Tried: `preferred-width: 0` plus `horizontal-stretch` columns, relying on the layout for row heights.
Outcome: rows too short: text overlapped the next row and the block clipped its last line.
Evidence: commit 9957627; filed upstream as slint-ui/slint#12776 with a standalone repro.
Hypothesis: `box_layout_info_ortho` merges child constraints computed at the wrong or unknown cross width.
Next time: explicit geometry: column widths as weight fractions × block width, and per cell `min-height: <text>.preferred-height + padding` (a wrapped Text's preferred-height tracks its actual width). Verified via the `md!` demo hook plus MCP screenshots.
Revisit when: #12776 lands (a measure-based cross-axis pass); then simplify TableBlock.
Cost: not recorded.
Scope: global-candidate     Status: active

## 2026-08-04 · StyledText limits: markdown subset, no selection, no accessibility · #slint #text slint/slinty-pi
Situation: rendering assistant markdown in Slint 1.17.
Tried: `StyledText`/`@markdown` for whole messages; osascript accessibility-tree checks for prose.
Outcome: no fenced code blocks, headings, tables or images; no selectable styled text (slint#736); StyledText content is absent from the macOS accessibility tree.
Evidence: recorded in project memory by 2026-08-04; led to pi-render's segmenter.
Next time: segment markdown first (pulldown-cmark) and use StyledText only for prose; copy buttons via arboard; verify prose by screenshot, not osascript. Verified.
Revisit when: Slint adds block-level markdown, text selection or StyledText accessibility.
Cost: the segmenter and highlighter had to be written.
Scope: global-candidate     Status: active

## 2026-08-03 · femtovg renderer slower than Skia on macOS · #slint #perf slint/slinty-pi/Cargo.toml
Situation: transcript-heavy streaming frames with Slint's default renderer.
Tried: default femtovg (glutin/OpenGL).
Outcome: measured about 30–40% more main-thread draw time than Skia/Metal on the same streaming load.
Evidence: switched in 2e25b9d (`renderer-winit-skia`); the winit backend prefers Skia when compiled in.
Hypothesis: OpenGL is a deprecated compatibility layer on macOS, and femtovg re-tessellates paths on the CPU every frame.
Next time: enable `renderer-winit-skia` for desktop apps. Verified by measurement.
Revisit when: femtovg moves to wgpu/Metal or Slint changes its defaults.
Cost: not recorded.
Scope: global-candidate     Status: active

## 2026-08-01 · Slint MCP cannot drive everything · #slint #testing #mcp
Situation: driving slinty-pi through the embedded Slint MCP server.
Tried: `set_element_value` on the composer `TextEdit` (and its inner TextInputs); simulating a Finder file drop.
Outcome: `set_element_value` silently no-ops on TextEdit; OS file drops have no Slint-core event MCP can inject.
Evidence: 2026-08-01 (project memory); file-drop gap reconfirmed 2026-10-04.
Next time: send scripted messages with `SLINTY_SEND_AFTER`, attach files with `SLINTY_ATTACH_AFTER`, and have a person do one real drag for drop changes. Verified.
Revisit when: Slint MCP gains text-entry or OS drag-and-drop injection.
Cost: not recorded.
Scope: global-candidate     Status: active

## 2026-10-04 · Slint MCP call shapes that failed first · #slint #testing #mcp
Situation: screenshots and hover checks via the MCP JSON-RPC endpoint (Slint 1.18.1).
Tried: `take_screenshot` with `windowHandle: {}` (meaning index 0); `move_pointer` with top-level `x`/`y`.
Outcome: "Invalid handle"; "unknown field `x`".
Evidence: 2026-10-04 symbol checks.
Next time: take the handle from `list_windows` (it was `{"index":"1","generation":"1"}`); `move_pointer` takes `position: {x, y}` in logical pixels; screenshots are 2x on Retina. Responses are plain JSON, not SSE. Verified.
Revisit when: the MCP tool schemas change.
Cost: two failed calls.
Scope: global-candidate     Status: active

## 2026-10-04 · "✕ ✎ ◷" drew as empty boxes with SF Pro · #slint #fonts #glyphs slint/slinty-pi/ui
Situation: after switching `default-font-family` to "SF Pro" (6a929ea), Slint 1.18.1, Skia renderer, macOS.
Tried: Unicode symbols as icon text: chip remove `✕`, close buttons, rename `✎`, palette `◷`, steer `↪`, settings `⚙︎`.
Outcome: rendered as empty boxes: `✕ ✎ ◷ ◴ ◵ ◔ ☰ ✐ ✑ ⇢ ⎘`. Rendered as colour emoji: `↪ ⚙︎ ✏ ✒ ✍ ⏱ ⌚` (`⚙︎` despite the U+FE0E text-presentation selector). Fine: `× ◇ ▸ ▾ · ✓ » ⚠ ● ○ — ◎ ⊙ ≡ ⋯ ⋮ ↻ → ⌘`.
Evidence: 2026-10-04, symbol rows rendered in the real app and MCP screenshots; fixes 380c68a, 1958123.
Hypothesis: font fallback doesn't reach a text font with these code points, and the selector is ignored; not verified, nor whether the boxes predate SF Pro.
Next time: before using a symbol as an icon, render it in the app (temporary placeholder text plus MCP screenshot) and pick from the verified set; for real icons use an SVG icon set. Verified approach.
Revisit when: Slint's font fallback or variation-selector handling changes, or the app bundles an icon font or SVG icons.
Cost: three rounds of render-and-screenshot.
Scope: global-candidate     Status: active

## 2026-10-04 · internal CustomApplicationHandler believed the only way to see file drops · #slint #winit #dnd slint/slinty-pi/src/main.rs
Situation: OS file drag-and-drop into the composer (Slint 1.18.1, winit 0.30).
Tried: `i_slint_backend_winit::CustomApplicationHandler` via `set_platform`, documented as "the only way", which needed an exact `=1.18.1` pin on an internal crate.
Outcome: not needed: the public `WinitWindowAccessor::on_winit_window_event` (slint feature `unstable-winit-030`) sees `HoveredFile`/`DroppedFile` before Slint's own handling; yapper used it all along.
Evidence: Slint 1.18.1 `dispatch_winit_window_event` applies the filter first; real Finder drop verified 2026-10-04; switched in 039052f.
Next time: check public `slint::winit_030` hooks (and sibling Slint apps) before depending on internal Slint crates. Verified.
Revisit when: Slint's winit 0.31 port changes the hook or handles external drops in `DropArea`.
Cost: an exact pin on an internal Slint crate from 2026-07-26 (43c56e2) to 2026-10-04, about ten weeks; one documented false constraint.
Scope: global-candidate     Status: active

## 2026-10-04 · demo backend made a working drop look broken · #testing #demo crates/pi-core/src/backend.rs
Situation: verifying the new file-drop hook with `SLINTY_DEMO=1`.
Tried: a real Finder drop onto the demo app.
Outcome: nothing happened; `demo_backend` drops `UiCmd::AttachPath`/`SetDragHover` in its `_ => continue` arm. The same drop in real `pi` mode produced the attachment chip.
Evidence: 2026-10-04 user report ("drop from finder doesn't do anything yet"), then real-mode screenshot.
Next time: before testing a UI path in demo mode, check that `demo_backend` handles its `UiCmd`s; test attachments in real mode. Verified.
Revisit when: demo_backend handles attach and drag-hover.
Cost: one false failure report and a relaunch.
Scope: project     Status: active

## 2026-10-04 · from yapper: headless backend differs on element lifetime · #slint #testing #mcp
Situation: yapper verifying per-row timers and change handlers via MCP with `SLINT_BACKEND=headless` (Slint 1.18).
Tried: debugging row heartbeats in headless.
Outcome: replaced delegates stayed alive and reported stale ids in headless; the real macOS backend was correct.
Evidence: yapper/bummer.md, 2026-09-26 "headless MCP backend trusted for row-lifetime behaviour".
Next time: verify lifetime-dependent behaviour (timers, change handlers, recycling) on the real backend; use headless only for layout and screenshots. Verified in yapper.
Revisit when: headless matches the real backend on delegate lifetime.
Cost: about six build-and-run rounds in yapper.
Scope: global-candidate     Status: active

## 2026-10-04 · from yapper: one-shot ListView scroll-to-end falls short · #slint #listview
Situation: yapper scrolling a long Slint 1.18 ListView to the newest row.
Tried: setting `content-y` to the end once, then twice.
Outcome: the last row stayed partly hidden.
Evidence: yapper/bummer.md, 2026-09-26 "one-shot scroll to the list end fell short" (recurred for sidebar reveals).
Hypothesis: content height is an estimate that grows as rows near the end are instantiated.
Next time: repeat the step over several frames (yapper: 5 steps, 40–450 ms). Verified in yapper. Applies to slinty-pi's `scroll-to-end()`.
Revisit when: Slint gets scroll-to-item / exact ListView content height.
Cost: three fix-and-test cycles in yapper.
Scope: global-candidate     Status: active

## 2026-10-04 · from yapper: key routing by a mirrored focus flag lost keys · #slint #focus #input
Situation: yapper routing keys in a root `capture-key-pressed` by a Rust flag mirrored from `changed has-focus`.
Outcome: keystrokes lost or taken for commands in fast sequences.
Evidence: yapper/bummer.md, 2026-09-26 "key routing by a mirrored focus flag lost keystrokes".
Hypothesis: `changed` handlers run asynchronously, so the mirror lags real focus.
Next time: route keys by where Slint's focus actually is (bubbling handlers, the widget's own `key-pressed`). Verified in yapper. Relevant to slinty-pi's global `capture-key-pressed` shortcuts.
Revisit when: Slint offers synchronous focus callbacks.
Cost: several stress-test cycles in yapper.
Scope: global-candidate     Status: active

## 2026-10-04 · from yapper: MCP element trees cap at 1000 elements · #slint #testing #mcp
Situation: yapper querying the whole-window element tree in a large layout.
Outcome: truncated at 1000 elements; existing controls looked missing.
Evidence: yapper/bummer.md, 2026-09-28 "whole-window MCP trees truncate in the actual tiled layout".
Next time: use targeted descendant/type/ID queries; don't read a truncated tree as a missing control. Verified in yapper.
Revisit when: MCP traversal limits change.
Cost: two interrupted verification runs in yapper.
Scope: global-candidate     Status: active

## 2026-10-04 · from yapper: wrapped StyledText links hit-test on the last line only · #slint #links #text
Situation: yapper making message links clickable with Slint 1.18.1.
Outcome: wrapped links responded only on their last line; plain text in linked paragraphs consumed clicks.
Evidence: yapper/bummer.md, 2026-09-28 "native link hit-testing has limits in released Slint" (source: `link_in_layout` overwrites per-line results).
Next time: expect this in slinty-pi's ProseRow links too; select messages via header/background. Verified in yapper.
Revisit when: a Slint release fixes wrapped-link hit-testing.
Cost: several pointer probes in yapper.
Scope: global-candidate     Status: active

## 2026-10-04 · from yapper: native context menu blocks MCP · #slint #testing #mcp
Situation: yapper right-clicking a TextInput via MCP on macOS (Slint 1.18.1).
Outcome: the call timed out while the synchronous native menu was open.
Evidence: yapper/bummer.md, 2026-09-29 "native context menu blocks Slint MCP inspection".
Next time: test menu paths by hand; drive the keyboard path via MCP. Suggestion.
Revisit when: MCP can drive native menus.
Cost: one timed-out call and an app restart in yapper.
Scope: global-candidate     Status: active

## 2026-10-04 · from yapper: MCP Command modifier is internal Control on macOS · #slint #testing #mcp
Situation: yapper sending Cmd+A / Cmd+Z through MCP.
Outcome: injecting Slint's internal Meta inserted literal "a"/"z"; internal Control worked.
Evidence: yapper/bummer.md, 2026-09-27 "native modifier-key automation was inconclusive", recurred 2026-09-28.
Next time: send `control` for macOS Command shortcuts via MCP. Verified in yapper.
Revisit when: Slint's modifier mapping changes.
Cost: a few calls in yapper.
Scope: global-candidate     Status: active

## 2026-10-04 · from yapper: Tooltip under a click area got no hover · #slint #tooltips
Situation: yapper attaching Slint's Tooltip to an element below a full-size TouchArea.
Outcome: no tooltip on hover.
Evidence: yapper/bummer.md, 2026-10-02 "tooltip below a tab click area did not receive hover".
Next time: attach the Tooltip to the TouchArea's parent. Verified in yapper. Same mechanism as the "trailing hover TouchArea" entry above.
Revisit when: Slint changes tooltip hit-testing.
Cost: one build cycle in yapper.
Scope: global-candidate     Status: active

## 2026-10-04 · from yapper: app counters don't prove Slint released images · #slint #memory #images
Situation: yapper planning lazy timeline images and measuring release via its own interest counters.
Outcome: Slint clones image handles into delegate and renderer caches; app-side counters can't observe them.
Evidence: yapper/bummer.md, 2026-10-03 "image interest counters do not prove toolkit memory release".
Next time: measure with native allocation traces and process memory snapshots. Suggestion.
Revisit when: Slint exposes image ownership instrumentation.
Cost: a plan revision in yapper.
Scope: global-candidate     Status: active

## 2026-10-04 · from yapper: MCP before/after screenshot comparisons needed care · #slint #testing #mcp
Situation: yapper checking .slint refactors for "no visible change".
Outcome: wall-clock timestamps caused false diffs; element queries returned nothing without debug info; a later non-MCP build overwrote the MCP binary.
Evidence: yapper/bummer.md, 2026-09-30 "Slint MCP before/after screenshots needed three retries".
Next time: build both sides with `SLINT_EMIT_DEBUG_INFO=1 --features slint/mcp` in separate target dirs, run back to back, and query roles by MCP names (`ListItem`). Verified in yapper.
Revisit when: a screenshot-compare script with pinned clocks exists.
Cost: about five build-and-run rounds in yapper.
Scope: global-candidate     Status: active

## 2026-10-04 · from Flectar Mail: SystemTrayIcon in the main compilation unit leaked a window · #slint #tray
Situation: Flectar Mail (Slint 1.18.1, `system-tray` feature) adding a tray icon.
Outcome: per a code comment there, compiling `SystemTrayIcon` with the main UI leaked an empty window; Flectar compiles `ui/tray.slint` as a separate unit in build.rs.
Evidence: second-hand: reported by a code inventory of ~/Code/flectar/mail on 2026-10-03; not re-verified here.
Next time: if slinty-pi or yapper adds a tray icon, try the separate compilation unit first; verify the leak before relying on this. Suggestion.
Revisit when: verified or contradicted in a Slint 1.18+ app.
Cost: none here yet.
Scope: global-candidate     Status: active

## 2026-10-04 · interactive cp/mv/rm aliases stalled tool calls in three projects · #shell #tooling
Situation: non-interactive tool calls on this machine; the shell profile aliases `cp`, `mv` and `rm` to their `-i` forms.
Tried: `cp backup original` to restore a file here (2026-10-04); `mv f.orig f` in yapper (2026-09-26); `rm` of WAL files in Flectar Mail (2026-10-03).
Outcome: each command waited on an overwrite/remove prompt: here the call hung until stopped and the file kept a temporary test; in yapper it hit the 300 s timeout; in Flectar nothing was deleted and a test ran on the wrong premise.
Evidence: this session's stalled `cp` ("overwrite …? (y/n [n])"), yapper/bummer.md 2026-09-26, flectar/mail bummer.md 2026-10-03.
Next time: use `command cp -f`/`command mv -f`/`command rm -f`, `git checkout --` for tracked files, or a Python copy; check the result afterwards. Verified (Python copy and `git checkout --` here).
Revisit when: the aliases are removed from the shell profile, or a hook rewrites these commands.
Cost: one hung call plus a restore here; a 300 s timeout in yapper; an invalid test run in Flectar.
Scope: global-candidate     Status: active
recurred: the global CLAUDE.md already warns about `cp` (2026-10-04); a repeat despite an instruction means tooling (a hook), not more text.
