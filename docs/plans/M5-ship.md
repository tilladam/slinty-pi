# M5 — Ship it

**Goal:** 1.0 quality, packaged, distributed. A stranger installs from a download link and
succeeds; performance budgets are enforced, not aspired to.

## Design

### 1. Settings surface

- Settings overlay (palette: "Settings", Cmd+,) with progressive disclosure — the LM Studio
  lesson: plain page first, "Advanced" reveals the rest.
  - **General**: default project behavior (reopen last / ask), density default, auto-scroll.
  - **Appearance**: theme follow-system/dark/light (sets `Palette.color-scheme` override),
    syntect theme pair, mono font, font size.
  - **Models**: opens the M3 panel; auto-compaction & auto-retry toggles (RPC `set_auto_*`).
  - **Permissions**: M4 mode + per-project decision list with revoke.
  - **Notifications**: banner on/off, sounds on/off.
  - **Advanced**: pi binary path override, extra CLI args, session dir override, log level.
- App state file (`directories`): window geometry, sidebar width, density, recents — atomic
  writes, versioned schema with lenient migration.
- Extension/skill/package listing (read-only + enable/disable): parse `pi list` output or
  settings.json `packages`; deep management stays in pi.

### 2. Final composer & transcript polish

- `@file` autocomplete in the composer: on `@`, fuzzy file picker over the project tree
  (respecting .gitignore via the `ignore` crate); inserts relative path; pi handles the rest.
- Session HTML export button (RPC `export_html`) + "Reveal session file" (transparency feature).
- Empty/error states audit: pi missing, provider down, router gone, session corrupt, child
  crashed (auto-restart offer) — every state has a designed card, not a bare error row.
- Animation audit: 150–200 ms ease-out on overlay/dialog entry, streaming shimmer, status
  transitions; nothing loops while idle (battery).
- Accessibility pass: full keyboard reachability, focus rings, contrast ≥ 4.5:1 for text (both
  schemes), accessibility labels on icon-only buttons (Slint `accessible-*` properties).
  Known ceiling documented: StyledText content is not exposed to VoiceOver (upstream slint
  limitation; transcript copy actions are the mitigation).
- Keybinding map: single source of truth in Rust (drives both `capture-key-pressed` routing and
  a "Keyboard shortcuts" cheatsheet overlay on Cmd+/).

### 3. App icon & identity

- Icon v1: simple geometric mark (π glyph in a rounded square, accent color), exported to
  `.icns`/`.ico`/PNG set; used for window, tray (template image variant), bundle, notifications.
- Name check: keep "slinty-pi" (repo) with display name "Slinty" — decide before notarization
  (bundle id `dev.tilladam.slinty-pi`).

### 4. Packaging & distribution

macOS (primary):
- `cargo-bundle` → `.app` with Info.plist (bundle id, category DeveloperTool, min macOS,
  `NSSupportsAutomaticGraphicsSwitching`, notification usage string).
- Universal binary (`lipo` of aarch64 + x86_64 targets).
- Sign bottom-up with hardened runtime (`codesign --timestamp --options=runtime`, no `--deep` —
  the flow already community-verified for Slint apps), `notarytool submit --wait`, `stapler`.
- Distribution: GitHub Releases (zip + dmg via `create-dmg`), Homebrew cask in a personal tap.
- Update check (not auto-update in 1.0): poll GitHub releases API on launch (≤ 1/day), unread-
  badge in settings + toast with release notes link. Full Sparkle-style auto-update post-1.0.

Linux: AppImage via `cargo-bundle`/`linuxdeploy`; verify fontconfig/wayland/x11 in a clean
container; document Ollama-first onboarding (llama.cpp router equally fine).

Windows: best-effort CI build + zip; smoke-tested in a VM once per release; known-issues doc.

### 5. CI & quality gates (GitHub Actions)

- PR gate: `cargo fmt --check`, `clippy --workspace -D warnings`, `cargo test --workspace`
  (pi-rpc integration tests auto-skip without pi), macOS + Linux runners.
- Nightly: install pi (`npm i -g`), run pi-rpc integration tests against latest pi release —
  the protocol-drift tripwire (product plan risk).
- Perf gates, honest and automatable:
  - Criterion micro-benches with thresholds: segmenter on a 50 KB mixed document (< 2 ms),
    highlighter on a 500-line file (< 15 ms), both asserted in CI against baseline JSON.
  - Headless soak: demo backend at 2000 tok/s × 60 s into a windowless harness (Slint software
    renderer + `slint::platform` test backend if workable, else the row-sync layer with a stub
    Ui) — asserts no unbounded memory growth and flush cadence ≥ 25 Hz.
  - Startup: release binary `--version`-to-first-frame measured on the macOS runner, budget
    < 1 s (tracked, warning-only on shared runners).
- Release workflow: tag → build/sign/notarize (secrets: Developer ID cert) → dmg/zip/AppImage →
  GitHub Release draft with changelog from conventional commits.
- 1k-message session soak: scripted resume of a generated 1k-message fixture, budgeted
  hydration time (< 1.5 s release) asserted in a test.

### 6. Docs & launch

- README rewrite: screenshots (light/dark), install (brew cask / download / cargo), 60-second
  quickstart, FAQ (local models, permissions, where sessions live).
- `docs/`: user guide (sessions, models panel, permissions, shortcuts), troubleshooting.
- Announce path: pi Discord + show-and-tell, r/LocalLLaMA, HN Show; collect the 3–5 community
  validation users promised in the product plan's success criteria before calling it 1.0.

## Work breakdown

1. Settings overlay + state file + keybinding registry & cheatsheet.
2. `@file` autocomplete + export/reveal + empty/error-state audit.
3. Animation + accessibility pass.
4. Icon + bundle identity.
5. cargo-bundle + sign/notarize scripts (`just release-mac`), dmg, cask.
6. CI: gates, nightly pi-drift job, perf benches, soak tests, release workflow.
7. Linux AppImage + container verification; Windows best-effort job.
8. Docs, screenshots, community validation round, 1.0 tag.

## Risks

- **Notarization friction** (first Developer ID setup, entitlements for child processes):
  budget a full day; the child-spawning (`pi`, node) usually needs no extra entitlements
  without sandboxing — App Sandbox is explicitly off for 1.0 (document why).
- **Perf gates on shared CI runners** are noisy: absolute budgets only for pure-CPU benches;
  wall-clock UI numbers tracked as trends, gated only on egregious regression (>2×).
- **Update-check privacy**: GitHub API call is the only network touch the app itself makes;
  document it and make it disableable.
- **Scope gravity**: M5 attracts feature creep; anything not on this page moves to post-1.0.

## Acceptance criteria (1.0 definition of done)

- Download → drag to /Applications → launch (no Gatekeeper warning) → onboarding → first local
  token, on a machine that has never seen the app, in under 5 minutes including model download.
- Cold start < 1 s; 60 fps while streaming; idle RAM < 150 MB (excluding models) — measured and
  recorded in the release notes.
- All CI gates green; nightly pi-drift job green against current pi release.
- Product-plan §9 validation: 3–5 pi community users complete a real task and prefer it to the
  TUI for review-heavy work.
