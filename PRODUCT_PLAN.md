# slinty-pi — Product Plan

A native desktop app, built with **Rust + Slint**, that drives the **pi coding agent** with a focus on
**local LLMs**. It replaces the browser/JS-based pi frontends (pi-gui, pi-desktop, pi-web) with
something that is fast the way native software is fast, simple the way good tools are simple, and
visually delightful without being busy.

*Status: product plan / requirements baseline — July 2026. Based on pi 0.81.x (RPC protocol) and
Slint 1.17.*

---

## 1. Vision

> **Delegate to a local agent as easily as you'd chat with a friend — and feel great doing it.**

The Claude Code desktop app and the OpenAI Codex app defined the product category: an *agent
dashboard* rather than an IDE. Both launched to the same loudest complaint — they are Electron apps
that drop frames on flagship hardware. Meanwhile the apps users praise unprompted for feel
(Supacode, Zed, ChatWise) win on one thing: **native speed you feel in every keystroke**.

slinty-pi takes the category-defining UX of those apps, marries it to pi's minimal, transparent
harness, and delivers it as a small, instant, native binary — with first-class support for running
**local models** (llama.cpp router, Ollama, LM Studio, vLLM) so the whole loop can run on your own
machine.

**Positioning in one line:** *the native cockpit for pi — chat-simple on the surface, agent-grade
underneath, local-first by design.*

## 2. Prior art — what we learned

### 2.1 pi (the harness we drive)

Researched from the locally installed pi 0.81.1 docs and the earendil-works/pi-mono repo.

- **RPC mode is the sanctioned contract for non-Node GUIs.** `pi --mode rpc` speaks JSONL over
  stdin/stdout (strict LF framing): commands (`prompt`, `steer`, `follow_up`, `abort`, `set_model`,
  `set_thinking_level`, `compact`, `bash`, `fork`, `clone`, `switch_session`, `get_entries`
  with durable cursors, `get_tree`, `get_session_stats`, `get_commands`, …) and a streaming event
  feed (`agent_start/end/settled`, `turn_*`, `message_update` with text/thinking/toolcall deltas,
  `tool_execution_start/update/end`, `queue_update`, `compaction_*`, `auto_retry_*`).
- **The extension UI sub-protocol is the interoperability key.** Extensions call `ctx.ui.select /
  confirm / input / editor / notify / setStatus / setWidget / setTitle`; in RPC mode these surface
  as `extension_ui_request` events the client must answer. A GUI that implements them gets the whole
  pi extension ecosystem — including the sanctioned permission-gating pattern (a `tool_call`
  extension + confirm dialog) — for free.
- **Sessions are an append-only JSONL tree** (`~/.pi/agent/sessions/--<cwd>--/<ts>_<uuid>.jsonl`,
  version 3): entries with `id`/`parentId`, in-place branching, compaction checkpoints, labels,
  branch summaries. The format is documented and stable — a GUI can read session files directly for
  browsing/search and use RPC `get_entries --since` for live sync.
- **Local LLMs are first-class**: llama.cpp router support (load/unload GGUF models, Hugging Face
  download via the TUI `/llama` command), and Ollama / LM Studio / vLLM / SGLang via
  `~/.pi/agent/models.json` custom providers with compat flags. Plus ~30 cloud providers and OAuth
  subscriptions via `auth.json`.
- **Philosophy to respect**: minimal core, no MCP (CLI tools + READMEs instead), no built-in
  permissions (extensions/sandboxing are the user's job), full context transparency — *nothing
  injected behind the user's back*. A GUI must surface everything (thinking, tool calls, compaction)
  and never hide context manipulation. Extensibility over baked-in workflow.
- **Gaps a GUI must fill itself**: TUI-only slash commands (`/settings`, `/login`, `/llama`,
  `/model` pickers) are *not* reachable over RPC — the app must reimplement settings, auth and
  llama.cpp model management against the files (`settings.json`, `auth.json`, `models.json`) and the
  router's HTTP API. TUI themes don't apply either; the app owns its visual identity.

### 2.2 Existing pi frontends (what we replace, and why)

| Frontend | Stack | Takeaway |
|---|---|---|
| **pi-gui** (minghinmatthewlam) | Electron 34 + React 19, pi SDK in-process | Feature-rich (worktrees, terminal, diff viewer, orchestration) but heavy, Node-coupled, Electron feel |
| **pi-desktop** (gustavonline) | Tauri 2 + Lit, Rust bridge → `pi --mode rpc` | Validates our exact architecture (subprocess + RPC); still a webview UI |
| **pi-web** variants | Astro/Go, session-file viewers | Mostly viewers or need a server; not a desktop experience |

Nobody has built a fully native frontend. That's the open slot.

### 2.3 Desktop agent app UX (Claude Code desktop, Codex, Conductor, Zed, Warp, Supacode…)

The patterns that most drive perceived quality, distilled and prioritized:

1. **Native speed** — zero input latency and a fast token-streaming path *are the brand*.
2. **Sidebar as attention dashboard** — sessions with live status (running / **needs you** / done),
   attention-sorted so blocked items surface to the top.
3. **Notify only when it matters** — a non-focused session finishes or needs input; never the one
   you're watching.
4. **Diff review as a loop, not a gate** — stats chip → diff viewer → inline comments batched back
   to the agent → accept/revert → commit.
5. **Permission UX with sticky memory** — visible mode, Allow once / Always / Deny, no prompt fatigue.
6. **Transcript density control** — Verbose / Normal / Summary; collapsed tool-call chips with
   elapsed time and token counts.
7. **Message queueing + steering** while the agent runs (Enter queues, visible editable queue).
8. **Keyboard-first** — command palette, one-keystroke model picker; these apps are judged like editors.
9. **Small "alive" details** — context-usage ring, per-session sidebar spinners, elapsed-time
   counters, hardware-honesty labels ("Fits / May be slow / Won't fit" à la Jan).
10. **Onboarding to first token in under a minute** (Jan auto-downloads a starter model) and
    **progressive disclosure** (LM Studio's User / Power User / Developer modes).

Failure modes to design against: pane sprawl, sidebar collapse past ~10 sessions, permission-prompt
fatigue, notifications without an inbox, feature sprawl (Msty).

### 2.4 Slint (the canvas we paint on)

Slint 1.16/1.17 crossed the desktop-readiness threshold: Skia renderer with subpixel glyph
positioning (excellent font rendering), auto dark/light with `Palette.color-scheme`, native macOS
menus/context menus, system tray, tooltips, drag & drop, and — crucially — `StyledText` with
markdown (`StyledText::from_markdown` at runtime) and read-only `TextInput`/`TextEdit` with caret +
mouse selection. Async Rust interop is best-in-class (`Weak::upgrade_in_event_loop`,
`invoke_from_event_loop`, `spawn_local` alongside tokio).

**Known gaps we must engineer around (see Risks):**

- `StyledText` markdown subset has **no fenced code blocks, headings, tables, or images** → we build
  a *markdown segmenter*: pulldown-cmark → segment model (paragraph / code block / heading / list) →
  StyledText for prose + custom components (monospace panel + syntect highlighting) for code.
- **No selectable styled text, no transcript-wide selection** (#736) → per-message and per-code-block
  Copy buttons (via `arboard`), read-only TextEdit where raw selection matters. Accept this ceiling;
  design so copying is *easier* than selecting.
- ListView with variable-height items jitters on append (#4097) → `Flickable` + repeater for the
  transcript (fine up to a few hundred messages) with manual pin-to-bottom; throttle streaming
  UI updates to ~30–60 ms batches.
- Frameless/custom-titlebar support is incomplete (#610/#613/#2521) → ship with the native titlebar
  + native menu bar; revisit custom chrome later.
- No existing Slint chat app to crib from — the message-rendering layer is genuinely new
  infrastructure (and reusable as a crate: a possible open-source halo for the project).

## 3. Product principles

1. **Instant.** Cold start < 1 s, first frame to first token with a loaded local model < 2 s,
   never a dropped frame while streaming. Perf is a feature with a budget, tested in CI.
2. **One window, one mental model.** Sidebar (sessions) + transcript + composer. No pane sprawl, no
   layout manager. Depth comes from progressive disclosure, not more chrome.
3. **Local-first, cloud-capable.** The happy path is a llama.cpp/Ollama model on your own machine;
   cloud providers work through the same picker. Hardware honesty everywhere.
4. **Transparent like pi.** Every message, tool call, thinking block, compaction and cost is
   inspectable. Nothing hidden, nothing injected. The transcript is the truth.
5. **Respect the harness.** Drive `pi --mode rpc` as a subprocess; never fork pi's logic. Implement
   the extension UI protocol so pi's ecosystem (permissions, plan mode, custom commands) just works.
6. **Delight is restraint.** A calm, warm visual identity (Conductor's lesson: "no more, no less");
   motion used sparingly for state changes (streaming shimmer, status transitions); typography does
   the heavy lifting. Dark and light, both first-class.
7. **Keyboard-first, mouse-friendly.** Command palette, model picker, session switcher on
   one-keystroke chords; everything also discoverable by pointing.

## 4. Target users

- **Primary:** developers running coding agents against local models (privacy, cost, offline, or
  tinkering) who live in pi's ecosystem or want an easier way into it.
- **Secondary:** pi TUI users who want a reviewing/monitoring surface with better rendering
  (markdown, diffs, images) than a terminal.
- **Tertiary:** local-LLM enthusiasts (LM Studio/Jan crowd) for whom pi's tools + a friendly GUI is
  the upgrade from "chat with a model" to "the model does things".

## 5. Scope

### In scope (v1 arc)

- Chat with streaming markdown, thinking blocks, collapsible tool-call chips, images.
- Session management: browse/resume/fork/name/delete, tree awareness, per-project grouping.
- Model management: pick pi-configured models; llama.cpp router panel (health, load/unload,
  HF download); models.json editing for Ollama/LM Studio/vLLM; thinking-level control.
- Steering/queueing, abort, compaction (manual + status), cost/context meters.
- Extension UI protocol: dialogs, notifications, status, widgets → native surfaces.
- Diff cards for edit-tool results with a proper diff viewer.
- Settings & auth surfaces for what RPC can't reach (settings.json, auth.json api keys).
- Notifications (only-when-it-matters), dark/light themes, command palette.
- macOS first (signed, notarized `.app`); Linux and Windows kept compiling throughout.

### Out of scope (v1)

- Multi-instance orchestration / parallel agents dashboard (pi-server is explicitly unstable;
  revisit when it stabilizes — the UI is architected so the sidebar can become an attention
  dashboard later).
- Git worktree management, embedded terminal, embedded editor (pi's bash tool + your real
  terminal/editor exist; Conductor/pi-gui already serve that niche).
- OAuth login flows for subscription providers (Claude Pro/ChatGPT) — v1 links out to `pi /login`
  in a terminal; revisit once we can reuse pi's flow non-interactively.
- Editing pi extensions/skills from the GUI (list + enable/disable only).
- Mobile/remote access.

## 6. Architecture (constraints for implementation planning)

```
┌────────────────────────────────────────────────┐
│ slinty-pi (Rust binary)                        │
│                                                │
│  ┌──────────┐   ┌───────────────────────────┐  │
│  │ Slint UI │◄──│ ViewModel (UI-thread state)│ │
│  └──────────┘   └────────────▲──────────────┘  │
│        ▲                     │ upgrade_in_event_loop
│        │              ┌──────┴───────┐         │
│        │              │ App core     │ tokio   │
│        │              │  - PiClient  │────────►│ pi --mode rpc (child, JSONL stdio)
│        │              │  - Sessions  │────────►│ ~/.pi/agent/sessions/**.jsonl (read)
│        │              │  - LlamaCtl  │────────►│ llama.cpp router HTTP (/health,/models)
│        │              │  - Config    │────────►│ settings.json / models.json / auth.json
│        └── markdown segmenter + syntect ───────│
└────────────────────────────────────────────────┘
```

- **Process model:** one `pi --mode rpc` child per open session; spawn/kill on session switch
  (v1: one live session at a time; the client layer supports N for the future dashboard).
- **Crate layout:** `pi-rpc` (typed protocol client: framing, commands, events, serde types —
  standalone, publishable), `pi-sessions` (JSONL tree reader/watcher), `slinty-pi` (app: viewmodel,
  segmenter, UI). The segmenter (`markdown → Vec<Segment>`) is its own module with golden tests.
- **Threading rule:** all UI mutation on the Slint thread via `Weak::upgrade_in_event_loop`;
  streaming deltas coalesced on a ~30–60 ms timer before touching models; update the last row via
  `set_row_data`, never reset the model.
- **llama.cpp integration:** talk to the router directly over HTTP for health/list/load/unload/
  download (the TUI `/llama` flow is not available via RPC), then `set_model` over RPC.
- **Renderer:** winit + Skia. **Style:** fluent as base with a custom design layer (own palette,
  spacing, components); cupertino evaluated for macOS feel in M1.

## 7. Milestones

Each milestone ends in something runnable and demoable. Ordering front-loads the two biggest
risks: RPC integration fidelity and Slint transcript rendering/performance.

### M0 — Skeleton & spike (de-risk) 
**Goal: prove the architecture end-to-end.**
- `pi-rpc` crate: spawn `pi --mode rpc`, strict-LF JSONL framing, typed commands/responses/events
  (serde), request-id correlation, integration tests against the real binary.
- Minimal window: input box + plain-text streaming transcript, abort button, model shown from
  `get_state`.
- Streaming perf harness: synthetic 100-token/s stream into the transcript, measure frame times.
- **Exit criteria:** chat with a local llama.cpp model through pi with zero dropped frames; go/no-go
  data on Flickable-vs-ListView for the transcript.

### M1 — A chat you'd choose to use
**Goal: the core conversation surface, beautiful.**
- Markdown segmenter (pulldown-cmark → segments) + StyledText prose + custom code-block component
  with syntect highlighting, language label, Copy button; headings, lists, links, images.
- Thinking blocks (collapsed by default, subtle animated indicator), streaming shimmer, per-message
  Copy; tool-call chips: name + one-line summary + elapsed time, expandable to full args/output,
  live-updating from `tool_execution_update`.
- Composer: multi-line editor, Enter/Shift-Enter, file/image attach (drag & drop), Esc = abort,
  Enter-while-streaming = steer (with visible queue chips from `queue_update`, editable).
- Model picker (Cmd+/) and thinking-level control in the composer; context-usage ring + session
  cost from `get_session_stats`.
- Design system pass: palette, type scale, spacing, dark/light, app icon v1.
- **Exit criteria:** daily-driveable single-session chat; transcript renders a 200-message session
  smoothly; design reviewed against the "calm, warm, restrained" bar.

### M2 — Sessions & projects
**Goal: never lose work; switch contexts instantly.**
- Sidebar: sessions grouped by project (cwd), read directly from session JSONL dirs; live row
  status (streaming spinner / idle / needs-input), name, relative time, cost; search (name +
  full-text over user messages); new/resume/fork/clone/delete (trash), rename via
  `set_session_name`.
- Project switcher (working directory picker with recents); `-c` continue-latest behavior on
  launch; session tree view (read-only v1): visualize branches, jump/fork from any user message.
- Transcript density toggle (Verbose / Normal / Summary) via Ctrl+O-style cycling.
- Command palette (Cmd+P): sessions, commands from `get_commands` (slash commands, skills,
  templates), actions.
- **Exit criteria:** all pi sessions on disk browsable and resumable; fork-from-message works;
  palette covers every action in the app.

### M3 — Local models, delightfully
**Goal: the differentiator — the best local-LLM front door pi has.**
- llama.cpp router panel: connection status, model list with loaded state, one-click load/unload,
  Hugging Face search + download with progress, RAM/VRAM **hardware-honesty labels** ("Fits / May
  be slow / Won't fit"), context-size display.
- models.json editor UI for Ollama / LM Studio / vLLM endpoints (guided form, not raw JSON;
  compat flags surfaced as friendly toggles); auth.json API-key entry for cloud providers
  (Keychain-backed where possible).
- **Onboarding flow:** first launch detects pi (offer install command if missing), detects
  running llama.cpp/Ollama, offers a starter model download → **first token in under a minute**.
- Provider/model status in the composer: local vs cloud badge, price or "free · local".
- **Exit criteria:** a fresh machine with only llama.cpp installed reaches a working agent chat
  without touching a terminal config file.

### M4 — The agent, trusted
**Goal: comfortable delegation — pi ecosystem interop, review loop, safety surface.**
- Extension UI protocol end-to-end: select/confirm/input/editor as native dialogs (with timeout
  handling), notify → in-app toasts + notification inbox, setStatus/setWidget → status-bar and
  composer-adjacent surfaces.
- Permission gating: ship/recommend a `tool_call`-gating pi extension; render its confirms as
  Allow once / Always / Deny cards with plain-English summaries and per-project sticky memory.
- Diff experience: edit-tool results as `+12 −1` stats chips → diff viewer (unified/side-by-side,
  syntax highlighted, from `details.patch`); bash tool output with streaming, exit-code badge,
  user-initiated `bash` command from the palette.
- Compaction UX: auto-compaction indicator, manual compact with custom instructions, "context
  compacted" transcript marker showing the summary; auto-retry status surfaces.
- Desktop notifications with the only-when-it-matters rule + inbox; system tray with status.
- **Exit criteria:** run a real multi-file coding task start-to-finish — permissions, diffs,
  steering, compaction — without wanting the TUI back.

### M5 — Ship it
**Goal: 1.0 quality, packaged, distributed.**
- Settings surface (general/appearance/models/notifications/advanced) with progressive disclosure;
  extension/skill/package listing with enable-disable (settings.json editing).
- Session HTML export (RPC `export_html`) + share affordances; `@file` autocomplete in composer.
- Polish pass: empty states, error states (pi missing, provider down, router gone), animation
  audit, accessibility pass (keyboard nav complete, contrast), full keybinding map + cheatsheet.
- Packaging: macOS universal build, cargo-bundle → codesign → notarize → staple, Sparkle-style
  update check (or brew cask); Linux AppImage/Flatpak; Windows installer (best-effort).
- Performance gate in CI (startup time, streaming frame budget) + soak test on 1k-message session.
- **Exit criteria:** a stranger installs from a download link and succeeds; perf budgets green.

### Post-1.0 themes (parking lot)
- **Attention dashboard:** multiple concurrent sessions with status badges and needs-you sorting
  (unlocked by our N-session client layer; watch pi-server stabilization).
- Diff review loop v2: inline comments on diff lines batched back to the agent.
- OAuth subscription logins in-app; session tree editing (labels, branch summaries).
- Reusable `slint-markdown` crate release; Windows/Linux first-class promotion.
- Voice input, global quick-entry hotkey window (Claude's double-Option pattern).

## 8. Risks & mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Slint text selection ceiling (no styled-text selection, no cross-message select) | Perceived polish gap vs Electron apps | Copy-first design (per-message/per-block copy, palette "Copy last response"); read-only TextEdit where raw selection matters; track slint#736/#9560 and adopt upstream when it lands |
| Transcript perf with variable-height rows (slint#4097) | Core-experience jank | M0 spike decides Flickable vs ListView; coalesced updates; density modes cap rendered content; virtualize/paginate history beyond ~500 messages |
| Markdown/code rendering is DIY | Schedule risk in M1 | Segmenter isolated behind golden tests; scope to CommonMark subset pi actually emits; tables/images can degrade gracefully (monospace fallback) at first |
| pi RPC protocol evolves (0.x, moving fast — 0.81→0.82 during research) | Breakage on pi updates | Version handshake on startup (`pi --version`), tolerant serde (unknown fields ignored), pin a tested range, CI matrix against latest pi release |
| TUI-only features (login flows, /llama) must be reimplemented | Scope creep | v1 links out to terminal for OAuth; llama.cpp management via router HTTP API is small and stable; settings editing scoped to known keys |
| pi-server instability blocks multi-session ambitions | Delayed differentiator | Keep v1 single-live-session; client layer designed for N sessions; revisit post-1.0 |
| One-person project vs big-team competitors | Sustainability | Ruthless scope (§5), reusable crates as community leverage, pi ecosystem alignment (extensions do the heavy lifting) |

## 9. Success criteria (v1)

- Cold start < 1 s; sustained 60 fps while streaming; idle RAM < 150 MB (excluding models).
- Fresh-machine onboarding to first local-model token < 60 s.
- A complete real coding task (multi-file edit, permission prompts, steering, compaction) is more
  pleasant in slinty-pi than in the pi TUI — validated with 3–5 pi community users.
- pi extension ecosystem works unmodified (dialogs, notifications, permission gating).
- Zero data loss ever: sessions are pi's files; we only append through pi.
