# M2 — Sessions & projects

**Goal:** never lose work; switch contexts instantly. All pi sessions on disk are browsable,
searchable, and resumable; forking works from any user message; a command palette covers every
action in the app.

## Verified facts this plan builds on

- Sessions live at `~/.pi/agent/sessions/--<cwd-with-slashes-as-dashes>--/<timestamp>_<uuid>.jsonl`
  (v3 JSONL: header line `{"type":"session","version":3,...}`, then tree entries with `id`/
  `parentId`; entry types `message`, `compaction`, `branch_summary`, `label`, `session_info`,
  `model_change`, …). Deleting a session = deleting its file; pi's own TUI uses the `trash` CLI.
- `pi-rpc` already implements the needed commands: `switch_session`, `fork {entryId}`, `clone`,
  `get_fork_messages`, `get_entries {since}`, `get_tree`, `get_messages`, `new_session`,
  `set_session_name`, `get_session_stats`.
- `switch_session` works within a running `pi --mode rpc` process; **changing project (cwd)
  requires respawning the child** (cwd determines session dir and context files).
- A `session_info` entry carries the display name; cost/token totals are derivable from message
  `usage` fields without asking pi.

## Design

### New crate: `crates/pi-sessions`

Read-only session index, independent of any running pi (pi stays the writer):

- `list_projects() -> Vec<Project>` — scan the sessions root, decode `--…--` dir names back to
  paths (lossy decode is fine: keep the raw dir name as identity, display the decoded path).
- `list_sessions(project) -> Vec<SessionMeta>` — parse header + scan entries cheaply for:
  display name (`session_info`), first user message (fallback title), entry count, last
  timestamp, summed cost/tokens. Cache per file keyed on (mtime, size).
- `load_session(path) -> SessionTree` — full parse into an id-indexed tree with the active leaf,
  for transcript hydration and the tree view.
- `search(query) -> Vec<Hit>` — substring/fuzzy over names + user-message text; brute-force over
  the cache first (typical corpora are small); no index files.
- `watch(root) -> impl Stream<SessionsChanged>` — `notify` crate watcher, debounced ~300 ms, so
  the sidebar tracks external `pi` TUI sessions live.

### Transcript hydration

Selecting a session must render its history without replaying events:

- `get_messages` after `switch_session` returns the active-branch `AgentMessage[]`.
- New `Transcript::hydrate(messages)`: map each message through the existing row machinery —
  user content → user row (+ image count badge), assistant `thinking` blocks → collapsed thinking
  row, assistant `text` → segmenter rows, `toolCall`/`toolResult` pairs (match on `toolCallId`)
  → finished tool chips (elapsed unknown → omit), `BashExecutionMessage` → tool-style row,
  compaction summaries → info row "context compacted · N→M tokens".
- Hydration happens off the UI thread; rows are pushed in one batched event-loop closure per ~100
  rows to keep the UI responsive on 1k-message sessions.

### Sidebar (new `ui/sidebar.slint`)

- Collapsible (Cmd+B), 260 px, `ListView` of sessions grouped by project; per-row: name (or first
  prompt, elided), relative time, cost; live status dot for the session bound to the running pi
  child (idle/streaming/needs-input — groundwork for the post-1.0 attention dashboard).
- Search field on top (filters via `pi-sessions::search`).
- Row actions (context menu, Slint 1.17 native `ContextMenuArea`): Resume, Fork…, Rename,
  Export HTML, Delete (→ `trash` crate, with confirm).
- "New session" (Cmd+N) and project switcher (recents + native folder picker via `rfd` crate)
  in the sidebar header. Project switch = kill child, respawn with new cwd, `-c`-style default:
  open latest session or empty state.

### Fork & tree view

- Fork dialog: `get_fork_messages` → list of user messages → pick → `fork {entryId}` → hydrate.
- Tree view (read-only v1): modal overlay rendering `get_tree` as an indented list; the active
  branch highlighted; clicking a node offers Fork-from-here. Branch labels/summaries shown when
  present. No graphical DAG in M2.

### Transcript density modes

Ctrl+O cycles Verbose / Normal / Summary (persisted per app):
- Verbose: everything, tool chips expanded by default.
- Normal (default): current behavior — collapsed chips, collapsed thinking.
- Summary: only user rows + assistant prose/heading/code; thinking, tool, info rows hidden.
Implemented as a `density` root property; row components collapse to zero height when filtered
(no model rebuild, so toggling is instant mid-stream).

### Command palette

- Cmd+P overlay: fuzzy list over (a) app actions (new session, switch project, density, compact,
  export, copy last response…), (b) sessions (resume), (c) pi commands from `get_commands`
  (slash commands, `skill:*`, prompt templates — invoking sends `/name` as a prompt).
- Single `Palette` component: text input + ranked `ListView`, keyboard-first (↑↓/Enter/Esc).
  Fuzzy matcher: `nucleo-matcher` crate (small, no service).
- Keyboard routing: root `FocusScope.capture-key-pressed` handles Cmd+P / Cmd+N / Cmd+B / Ctrl+O
  before the composer sees them.

### M1 stragglers folded in

- Drag & drop / attach button: `DropArea` over the composer; images → base64 `ImageContent` on
  `prompt`, small thumbnail chips above the composer; non-image files attach as `@path` text.
- Per-message copy: hover affordance on message groups (backend keeps each message's raw
  markdown; copy uses it, not rendered text).
- Pin-to-bottom only when at bottom: expose `at-end` from the transcript ListView; `Ui` closures
  skip `scroll-to-end` when the user scrolled up; "jump to latest" pill appears instead.

## Work breakdown

1. `pi-sessions` crate: dir scan, header/meta parse, cache, tests against fixture files copied
   from real sessions (anonymized), watcher.
2. Transcript hydration path + `get_messages` mapping tests (fixture JSONL → expected row kinds).
3. Sidebar UI + wiring: list, search, status dot, context menu, delete/rename/export.
4. Project switcher + child respawn lifecycle (graceful abort of in-flight stream, error rows on
   respawn failure).
5. Fork dialog + tree overlay.
6. Density modes + persistence (`directories`-based state file).
7. Command palette + keyboard routing.
8. Composer attach/drag-drop + per-message copy + scroll pinning.
9. Demo backend: synthesize a fake session dir in the scratchpad so sidebar/hydration are
   demoable without pi.

## Risks

- **Hydration fidelity** vs. live-streamed rendering (same message, two paths). Mitigation: one
  shared `message → RowSpec[]` function used by both hydration and a `turn_end` reconciliation.
- **Large sessions** (10k+ entries): meta scan stays O(file) but cached; full hydration batched;
  if still slow, hydrate only the last N messages with "load earlier" (explicitly acceptable).
- **cwd-encoded dir names** are lossy (`-` ambiguity). Never round-trip through the decoded
  string; use the raw dir name as the key.

## Acceptance criteria

- Every pi session on disk (including ones created by the TUI while the app runs) appears in the
  sidebar within a second, with sensible titles.
- Resume of a 500-message session renders in under a second, correctly segmented, and continues
  streaming seamlessly on the next prompt.
- Fork-from-message produces the same result as pi's TUI `/tree` fork (spot-check).
- The palette reaches every app action; the app is fully drivable without the mouse.
- Deleting a session moves the file to the Trash and never touches other files.
