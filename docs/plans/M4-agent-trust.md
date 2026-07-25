# M4 — The agent, trusted

**Goal:** comfortable delegation. Run a real multi-file coding task start-to-finish — permission
prompts, diffs, steering, compaction — without wanting the TUI back. The pi extension ecosystem
works unmodified.

## Verified facts this plan builds on

- Extension UI sub-protocol (rpc.md): dialog methods `select` / `confirm` / `input` / `editor`
  block the extension until the client answers `extension_ui_response` `{id, value|confirmed|
  cancelled}`; optional `timeout` auto-resolves agent-side (client needs no timer for
  correctness, only for UX). Fire-and-forget: `notify {message, notifyType}`, `setStatus
  {statusKey, statusText}`, `setWidget {widgetKey, widgetLines, widgetPlacement}`, `setTitle`,
  `set_editor_text`. `pi-rpc` already parses requests and can reply.
- pi has **no built-in permission system by design**; the sanctioned pattern is a `tool_call`
  extension that gates via `ctx.ui` dialogs. Whatever we ship must be a normal pi extension so
  TUI users get the same behavior.
- The `edit` tool result carries `details.diff` (TUI-rendered) and **`details.patch` — a
  standard unified diff for GUI consumers**. Tool results separate `content` (for the LLM) from
  `details` (for UIs).
- `bash` RPC command runs user-initiated shell commands whose output joins context on the next
  prompt (`bash_execution_update` streams output; `abort_bash` cancels).
- Compaction: `compact {customInstructions?}`, `set_auto_compaction`, events
  `compaction_start/end {reason: manual|threshold|overflow, result.summary, tokensBefore,
  estimatedTokensAfter}`.
- Slint 1.17 has `SystemTrayIcon`; window focus is observable (`Window.active`); native macOS
  notifications need a helper (no Slint API) — `mac-notification-sys` / `notify-rust`.

## Design

### 1. Extension UI protocol, end-to-end (the interop keystone)

- Modal layer in `app.slint`: one `DialogHost` overlay rendering the active request:
  - `select` → list with keyboard nav (number keys = quick select, matching pi TUI habits).
  - `confirm` → message + Yes/No (Enter/Esc).
  - `input` → single-line field with placeholder.
  - `editor` → multi-line TextEdit prefilled.
  - Requests queue FIFO; timeout shown as a thin countdown bar when present (agent auto-resolves
    anyway — the bar is honesty, not logic).
- `notify` → toast stack (top-right, auto-dismiss info after 5 s, warnings/errors sticky) **and**
  an entry in the notification inbox (see §5).
- `setStatus` → keyed segments in the status bar; `setWidget` → keyed mono-text block above or
  below the composer; `setTitle` → window title suffix. `set_editor_text` → composer text.
- Backend: `DialogRouter` owning the request queue; answers go through
  `PiClient::reply_extension_ui`. Everything demoable via demo-mode synthetic requests.

### 2. Permission gating (ships as a pi extension + first-class rendering)

- `assets/pi/permissions.ts`: a pi extension gating `tool_call` (bash/write/edit + extension
  tools) with per-project sticky memory in `.pi/permission-decisions.json`. Dialog title uses a
  structured prefix (`permission: bash`) plus plain-English body ("run `cargo test` in
  ~/project"), options exactly `["Allow once", "Always allow", "Deny"]`.
- Install flow: settings/onboarding offers "Enable permission prompts" → copies the extension to
  `~/.pi/agent/extensions/` (with consent, and only if absent); works identically in the pi TUI.
- The GUI special-cases dialogs whose title matches the structured prefix into a **permission
  card**: tool icon, command/path summary, the three options as buttons (1/2/3 keys). Unknown
  select dialogs still render generically — no lock-in.
- Mode surface: a composer selector (Ask / Allow edits / Allow all) that maps to the extension's
  sticky memory file; explicitly out of scope: OS-level sandboxing (documented pointer to
  containerized pi instead).

### 3. Diff experience

- Edit-tool chips become **stats chips**: parse `details.patch` → `+12 −3 path/to/file`; click
  opens the diff viewer.
- Diff viewer: overlay panel (not a window) with file header, unified/side-by-side toggle,
  syntect-highlighted lines on the code-card background, added/removed line tinting, per-file
  navigation when a turn touched several files (group chips by turn). Read-only in M4 —
  review-loop comments are post-1.0.
- Parser: hand-rolled unified-diff parser (small, fixture-tested; no heavy dep).

### 4. Bash & compaction surfaces

- Palette action "Run shell command" → `bash` RPC; output streams into a tool-style row
  (`bash_execution_update`), Esc/`abort_bash` cancels; a subtle "joins context on next prompt"
  hint on the row (that's pi's actual semantics).
- Compaction: palette + status-ring menu ("Compact now", optional instructions via input
  dialog); `compaction_start/end` render as an info row with reason and `tokensBefore →
  estimatedTokensAfter`; auto-compaction toggle in settings; overflow-retry status surfaced.
- Auto-retry events get a status-bar countdown with an inline "Stop retrying" (`abort_retry`).

### 5. Notifications & tray

- Rule (from the product plan): notify only when the window is **not** focused and either the
  agent settled or a dialog/permission request is waiting. macOS banner via
  `notify-rust`; clicking focuses the app (activation via `open -b` fallback if needed).
- In-app inbox: bell in the status bar with unread count; entries = notifies, settles while
  unfocused, permission requests, extension errors. Clearing is one click; no persistence
  across restarts in M4.
- `SystemTrayIcon`: state glyph (idle / streaming / needs-you), menu: Show window, current
  session name, Quit.

## Work breakdown

1. DialogHost + toasts + status segments + widgets; demo-mode request synthesizer; answer
   plumbing tests (fake pi echoing `extension_ui_response` lines).
2. Notification inbox + focus-aware banner rule + tray icon.
3. permissions.ts extension (developed against pi TUI first, then GUI card rendering + install
   flow + mode selector).
4. Unified-diff parser + stats chips + diff viewer overlay.
5. Bash palette command + streaming row + abort.
6. Compaction & retry surfaces.
7. End-to-end dogfood: scripted multi-file task exercised against a local model, checked for
   every §1–§6 surface.

## Risks

- **Dialog deadlocks**: an unanswered dialog blocks the extension; ensure abort/session-switch/
  child-death paths auto-cancel pending dialogs (and `DialogRouter` drops stale ids defensively).
- **Permission extension correctness** matters more than the GUI: test it standalone in pi TUI
  (its failure mode must be "asks too often", never "allows silently").
- **Patch parsing edge cases** (renames, mode changes, no-newline markers): fixture corpus from
  real pi edit results; fall back to raw patch text display on parse failure.
- **Notification permission** on macOS may require a signed bundle (M5); degrade to in-app inbox
  only, silently.

## Acceptance criteria

- A pi extension using `select`/`confirm`/`input`/`editor`/`notify`/`setStatus`/`setWidget`
  behaves identically under the GUI and the TUI (test with a purpose-built exerciser extension).
- With permissions enabled, `bash`/`edit`/`write` pause for a card; "Always allow" persists per
  project and pi TUI honors the same decisions file.
- An `edit` producing a 100-line patch shows a correct stats chip and a diff view matching
  `git diff` rendering of the same patch (fixture comparison).
- Backgrounding the app during a long run yields exactly one banner when it finishes or blocks —
  never while focused, never repeats.
