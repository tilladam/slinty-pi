# Shared crates across slinty-pi, yapper and Flectar Mail — plan

Status: plan, 2026-10-04. Phase 0 done: `tilladam/slint-kit` exists (private for now, D8) with
licence, policy and CI. Phases 2–4 extracted (`md-segments`, `emoji-shortcodes`,
`desktop-clipboard`, `desktop-notify`); the consumers haven't switched yet (D10, D11). Builds on the code inventory of the three
apps done on 2026-10-03 and the platform-glue review of 2026-10-04.

**How to read the evidence tags:**
- **[read]**: checked against the code or upstream source in the session that wrote this plan.
- **[inventory]**: reported by the 2026-10-03 inventory agents with file paths, not re-read.
- **Numbers** (line counts, effort) are estimates unless marked measured.

---

## 1. Decisions already made

| # | Decision | Source |
|---|---|---|
| D1 | Shared repos are **MIT-licensed and public on GitHub** (`github.com/tilladam/…`). | User, 2026-10-04 |
| D2 | Shared crates **depend only on Slint or other public crates** — no path or git dependencies on slinty-pi, yapper or anything private. | User, 2026-10-04 |
| D3 | Yapper's own code may be shared as MIT; `yapper-signal` and `spikes/sg-link` stay AGPL and must never be a dependency of shared code. | User, 2026-10-03; yapper `LICENSING.md` (2526883) |
| D4 | **Flectar Mail is ideas-only.** It is AGPL-3.0-only and mostly another author's work (git: 338 commits by its author, 4 by Till) [read]. Nothing is copied from it; ideas are reimplemented from a description, not from its source. | Analysis 2026-10-03 |
| D5 | Keep pure (toolkit-agnostic) code and Slint-dependent code in **separate crates from the start**. | Yapper's bummer.md, 2026-10-04 (user chose `yapper-app` + `yapper-view` over "one crate first"); applied here by analogy |
| D6 | **One repo**, a Cargo workspace holding all the crates (was O1). | User, 2026-10-04 |
| D8 | **Private for now**; made public later, when Till decides (D1 is the end state). | User, 2026-10-04 |
| D9 | **slint-kit holds code only.** This plan stays in slinty-pi, and the Slint gotchas stay in the apps' own `bummer.md` files; neither goes into slint-kit. | User, 2026-10-04 |
| D10 | **slinty-pi keeps its own copy** of the extracted code until slint-kit is public; a public repo must not depend on a private one. | User, 2026-10-04 |
| D11 | **Yapper's switch waits for its refactor** (another session is active there); copying out of yapper is read-only. | User, 2026-10-04 |
| D12 | **Notifications: extract yapper's `platform.rs` as `desktop-notify`**, with an opt-in `notify-rust` fallback off macOS (was O4; evaluation in phase 4). | User, 2026-10-04 |
| D7 | The repo is named **`slint-kit`** (`github.com/tilladam/slint-kit`; was O2). Read as "a kit for Slint apps": only the `slint-*` crates depend on Slint, the others are Slint-free companions (D5), and SwiftyPi may use them too. | User, 2026-10-04 |

**Already done (not part of this plan any more):**
- yapper `LICENSE` + `LICENSING.md` (2526883).
- slinty-pi dev-profile `opt-level = 3` for dependencies (bfa37cc).
- slinty-pi file drops via Slint's public `on_winit_window_event`, dropping the internal
  `i-slint-backend-winit` dependency and its `=` pin (039052f); links via `webbrowser` (9c342cc).
- Slint gotchas from all three apps collected in slinty-pi's `bummer.md` (2026-10-04).

## 2. Open decisions for Till

| # | Question | Recommendation |
|---|---|---|
| O3 | Consume via git tags only, or also publish to crates.io? | **Git tags first** (`git = "…", tag = "v0.x"`), crates.io once an API has survived both apps for a release or two. |

## 3. Bummer entries that apply

From slinty-pi `bummer.md`:
- **All Slint entries**: read before any `slint-*` crate work. They stay in the apps'
  `bummer.md` files, not in slint-kit (D9).
- **"internal CustomApplicationHandler believed the only way…"** → phase 6: use public
  `slint::winit_030` hooks only; no internal Slint crates in shared code.
- **"✕ ✎ ◷ drew as empty boxes with SF Pro"** → phase 7: SVG icons, not Unicode symbols.
- **"demo backend made a working drop look broken"** → phases 3 and 6: verify attach paths in
  real mode.
- **"Slint MCP cannot drive everything"** → phases 3 and 6 need one manual paste/drag check.

From yapper `bummer.md`:
- **"one shared crate first" declined** (D5 above).
- **"refused tool call still changed files"** and its recurrence, plus yapper's note about
  concurrent sessions in the same checkout → run `git status` in yapper before every
  migration step there.
- **"workspace formatting touched unrelated concurrent changes"** → format only touched
  files in consumer repos.
- **Slint testing entries** (headless lifetime, MCP tree cap, screenshot-compare recipe) →
  verification of phases 5 and 7.

## 4. Target layout

One public workspace repo, `slint-kit` (D6, D7), MIT, with pure and Slint crates separated (D5). Working names:

```
slint-kit/
  LICENSE (MIT)            deny.toml (license + source policy, see §5)
  crates/
    md-segments/           pure   markdown → segments, syntect highlighting, linkify   (phase 2)
    emoji-shortcodes/      pure   :shortcode: tables (iamcal, joypixels), zero deps     (phase 2)
    desktop-clipboard/     pure   clipboard image as encoded bytes                       (phase 3)
    desktop-notify/        pure   notifications + dock badge — only if O4 says extract   (phase 4)
    slint-model-sync/      slint  keyed VecModel reconcile + update coalescing           (phase 5)
    slint-file-drop/       slint  OS file drops with hover state and drop position       (phase 6)
    slint-widgets/         slint  .slint library: palette, code block, copy button,
                                  tokens, SVG icons                                     (phase 7)
    local-llm/             pure   rapid-mlx/llama.cpp/Ollama management, OpenAI-compatible
                                  streaming client                                       (phase 8)
```

Consumers:
- **slinty-pi**: `pi-render` re-exports `md-segments` (so SwiftyPi keeps getting it through
  `pi-core-ffi`); `pi-local` keeps the pi-specific parts (`auth_json`, `models_json`, `panel`).
- **yapper**: after its refactor, `yapper-view` depends on the pure crates and `yapper-ui` on
  the Slint ones.
- **Flectar Mail**: may depend on any of these (MIT into AGPL is fine); not a goal of this plan.

## 5. Phase 0 — bootstrap the repo

Steps:
1. Create the public GitHub repo `tilladam/slint-kit` with `LICENSE` (MIT, "Copyright (c) 2026 Till
   Adam"), a README stating D1/D2, and an empty workspace (`edition = "2024"`; consumers on 2021
   can still depend on 2024-edition crates).
2. CI (GitHub Actions) on macOS and Linux: `cargo fmt --check`, `cargo clippy --workspace
   -D warnings`, `cargo test --workspace`.
3. **Enforce D2 mechanically** with `cargo-deny` in CI:
   - `[sources] unknown-git = "deny"`, `unknown-registry = "deny"`: only crates.io.
   - `[licenses] allow`: MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0, MPL-2.0, plus
     `LicenseRef-Slint-Royalty-free-2.0` so Slint's `GPL-3.0-only OR LicenseRef-…` expression
     resolves to a permissive branch (to verify in the bootstrap PR that cargo-deny evaluates Slint's
     `OR` expression this way). No GPL/AGPL-only crates.
4. Slint policy for the Slint crates: `slint = { version = "1.18", default-features = false,
   features = [...] }` with only what each crate needs; `unstable-winit-030` only in
   `slint-file-drop`, behind a cargo feature of the same name.

Acceptance: an empty-crate PR passes CI including `cargo deny check`; a test PR adding a git
dependency fails it.
Effort: half a day (estimate).

**Done 2026-10-04** (slint-kit b9f1882). Differences from the steps above:
- Cargo can't run on a workspace with no members, so CI skips its cargo steps until the first
  crate exists; no placeholder crate.
- `BSL-1.0` joined the allow list (Boost; arboard → clipboard-win on Windows, via Slint).
- Policy verified locally with a throwaway probe crate (cargo-deny 0.20.2): Slint 1.18 alone
  passes (its `OR` licence resolves to the royalty-free branch), a git dependency fails
  `sources`, and a path dependency into slinty-pi fails the `cargo metadata` check.
- Pushing needed SSH (`git@github.com:`): the `gh` token lacks the `workflow` scope that adding
  `.github/workflows/` over HTTPS requires.

## 6. Phase 1 — dropped

Was: move the Slint gotchas into a `slint-kit/docs/slint-gotchas.md`. Dropped on 2026-10-04
(D9): they stay in slinty-pi's `bummer.md` (which also summarises yapper's Slint entries) and in
yapper's own `bummer.md`. Phase numbers below are kept as they were.

## 7. Phase 2 — `md-segments` and `emoji-shortcodes`

**Sources:**
- slinty-pi `crates/pi-render/src/segmenter.rs` (`segment_markdown`, `Segment`, `TableCell`)
  and `highlight.rs` (`highlight_lines`, `CodeLine`, `ColoredSpan`, `theme_background`) [read].
  Neither depends on pi types [inventory]. pulldown-cmark 0.13, syntect 5.3 (`default-fancy`).
- yapper `crates/yapper-ui/src/blocks.rs`: `links`, `linkify`, `emoji_shortcodes`,
  `literal_ranges`, `custom_emoji` [read: names]. These are string → string helpers.
  `parse`/`Spec` (yapper's own block model) and the attachment helpers stay in yapper.
- yapper `crates/yapper-emoji` (tables plus their MIT licence files) → `emoji-shortcodes`
  unchanged.

**API (sketch):** `segment_markdown(&str) -> Vec<Segment>`; `highlight_lines(code, lang, dark) ->
Vec<CodeLine>` behind a default `highlight` feature (syntect is the heavy dependency);
`linkify(&str) -> String`; `links(&str) -> Vec<String>`; emoji replacement taking a
`&emoji_shortcodes::Table`.

**Steps:**
1. Move the slinty-pi files with their tests; `pi-render` depends on `md-segments` and
   re-exports the moved items, so `pi-core`, `pi-core-ffi` and slinty-pi compile unchanged.
2. Move yapper's helpers and `yapper-emoji`, with tests. **Coordinate with yapper's refactor
   plan** (`plan/refactoring.md`, D4): `blocks.rs` is destined for `yapper-view`. Extract the
   helpers either before the refactor (smaller diff to move later) or as part of it, not
   halfway through.
3. Follow-up, not this phase: whether yapper's `Spec` block model and `Segment` should converge
   (yapper has list items and images, slinty-pi has headings, tables and rules). Decide after
   both apps use the shared helpers.

**What each app gains:** yapper gets syntax highlighting for code blocks, though its
`BlockView` needs coloured spans to show it (or the shared code-block widget from phase 7).
slinty-pi gets bare-URL linking and `:shortcode:` emoji in prose.

**Extracted 2026-10-04** (slint-kit ea92be5; provenance in the commit message):
- `md-segments` holds `segmenter`, `highlight` (feature `highlight`, default on) and `inline`
  (yapper's helpers). `emoji_shortcodes(src, &Table)` now takes the table, so yapper's
  wrapper passes `rocketchat()`. `literal_ranges` is public.
- `emoji-shortcodes` is yapper-emoji with the API unchanged and the data and licence files
  byte-identical.
- 25 tests moved (13 segmenter, 6 highlight, 5 inline, 1 emoji), all passing; clippy with
  `-D warnings`, fmt, cargo-deny and the path check are clean locally.

**Still to do: switching the consumers.**
- slinty-pi (once slint-kit is public, D10): `pi-render` depends on `md-segments` by tag and
  re-exports the *modules* (`pub use md_segments::{segmenter, highlight}`), so
  `pi_render::segmenter::…` paths keep compiling in pi-core, pi-core-ffi and SwiftyPi; then
  delete the two local files.
- yapper (with its refactor, D11): `yapper-emoji` becomes a re-export of `emoji-shortcodes`, so
  its six dependants don't change; `blocks.rs` keeps thin wrappers (`emoji_shortcodes` passing
  `rocketchat()`). Touch only those files; no workspace-wide `cargo fmt`.

Acceptance: all moved tests pass in the new repo; slinty-pi `cargo test --workspace` green;
yapper tests green; a slinty-pi `md!` demo render and a yapper code block look identical before
and after (MCP screenshots, per yapper's screenshot-compare recipe).
Effort: 1 day (estimate).

## 8. Phase 3 — `desktop-clipboard`

**Problem:** slinty-pi pastes images via `arboard::get_image()`, which decodes the whole image
to RGBA, and then re-encodes PNG (`pi_core::attach::encode_png`) [read]. So a huge screenshot
is fully decoded on paste and a JPEG becomes a PNG. Yapper reads the pasteboard's encoded
PNG/TIFF/JPEG bytes directly on macOS and decodes later on a bounded worker
(`attachment_input.rs` `clipboard_image`) [read].

**API (sketch):** `clipboard_image() -> Result<Option<EncodedImage { bytes, format }>>`. macOS:
NSPasteboard via `objc2-app-kit` (yapper's code). Elsewhere: `arboard` fallback, explicitly
documented as decoding.

**Steps:**
1. Extract yapper's `clipboard_image` (macOS) and its non-macOS stub into the crate; add the
   `arboard` fallback.
2. slinty-pi: replace the `get_image` + `encode_png` path. PNG/JPEG go to pi as-is. TIFF (common
   on macOS) needs conversion to PNG with bounded decoding (`image` crate `Limits`, as in
   yapper's `encode_clipboard`).
3. yapper: depend on the crate instead of its local copy.

**Extracted 2026-10-04** (slint-kit c90888c, CI green on macOS and Linux; provenance in the
commit message):
- `clipboard_attachment()` → `Files(Vec<PathBuf>)` or `Image(EncodedImage { bytes, format })`;
  `clipboard_image()`; `to_png(EncodedImage) -> Vec<u8>` (yapper's `encode_clipboard`, now
  returning bytes, same limits: 8192 px per side, 16 MP, 128 MiB); `EncodedImage::mime_type()`.
- Differs from step 1: off macOS, images read as `None` by default (yapper's policy against
  unbounded decoding). The `arboard` decode is the opt-in feature `decoding-fallback`, which
  slinty-pi will enable to keep pasting images on Linux and Windows.
- CI now runs clippy and tests with `--all-features` (so Linux compiles the fallback) and a
  `--no-default-features` build.
- Not yet verified: the macOS NSPasteboard read itself (no automated way to put an image on
  the clipboard). It is covered by the manual acceptance check below when a consumer switches.

Acceptance: a manual paste check of a screenshot (PNG), a copied JPEG and a Preview copy (TIFF)
in both apps. MCP can't inject OS clipboard images (bummer: "Slint MCP cannot drive
everything"). A unit test for format detection.
Effort: half a day to a day (estimate).

## 9. Phase 4 — notifications and dock badge

1. **Evaluate `user-notify`** (0.4.2 on crates.io [read: version only]) against yapper's
   `platform.rs`:
   - uses `UNUserNotificationCenter` on macOS;
   - click callback carrying an id;
   - behaviour when unbundled (yapper guards with `bundled()` because the API aborts the
     process without a bundle identity [inventory]);
   - Linux and Windows support;
   - licence.
2. If it passes: both apps use it directly, and only the dock badge (`NSDockTile`, about 20
   lines) needs a home, a module in `desktop-notify` or each app.
3. If it fails: extract yapper's `platform.rs` (225 lines [inventory]) as `desktop-notify`, with
   no-op stubs off macOS.
4. slinty-pi use cases: a turn finishes while the window is inactive; an extension dialog needs
   input (M4); the notification settings in M5 §1.

**Evaluation, 2026-10-04** (from the crates' source):

| Criterion | `user-notify` 0.4.2 | `notify-rust` 4.18.1 (+ `preview-macos-un` → `mac-usernotifications` 0.3.1) | yapper `platform.rs` |
|---|---|---|---|
| Licence | **LGPL-3.0-or-later ✗** (rejected by `deny.toml`; static linking makes LGPL awkward for MIT binaries) | MIT/Apache ✓ | MIT ✓ |
| macOS API | `UNUserNotificationCenter` ✓ | deprecated `NSUserNotification` by default; UN only behind a *preview* feature | `UNUserNotificationCenter` ✓ |
| Click → callback with an id | ✓ delegate | only while the app holds that notification's handle; clicks without one (launch, after restart) are dropped ("no pending sender") | ✓ one global callback, including the launch click |
| No bundle | mock fallback ✓ | `NoBundleIdentifier` error ✓ | guarded, off ✓ |
| Linux / Windows | ✓ | ✓ | no-ops |
| Badge, activate, withdraw by id | badge permission only | close by handle; no badge | ✓ all |

Decision D12: extract yapper's code. **Extracted 2026-10-04** (slint-kit 7dee607, fix d63dbe2;
provenance in the commit message):
- API: `install(app_name, on_click: Fn(String))` (raw id; yapper keeps its
  `notification_id`/`parse_notification_id`), `available()`, `notify(id, thread, title,
  subtitle, body)`, `withdraw(ids)`, `set_badge(n)`, `activate()`, `is_active()`.
- Off macOS: no-ops; feature `fallback` shows notifications through notify-rust (show-only).
- Diagnostics use the `log` facade instead of `eprintln!`. slinty-pi's `tracing-subscriber`
  picks them up; yapper needs a logger (or `env_logger`) to keep seeing them when it switches.
- The ObjC delegate class is `SlintKitNotificationDelegate`. A process must not install both
  this crate and yapper's own copy.
- The first CI run failed on Linux only: with `fallback` on, the test was compiled out but its
  `use super::*` stayed, and `-D warnings` rejected it. Fixed by gating the whole test module.
- Not yet verified: a real notification and click. That needs a bundled build, so it is
  covered by the acceptance check below when a consumer switches.

Acceptance: an evaluation note answering each criterion with evidence; a notification raised and
clicked in a bundled build of each app (cargo-bundle for slinty-pi, yapper's
`scripts/macos-runner.sh`).
Effort: half a day for the evaluation; 1 day for either outcome (estimate).

## 10. Phase 5 — `slint-model-sync`

**Problem:** slinty-pi rebuilds the transcript model when hydrating a session (batched
`push_all`, 100 rows per event-loop hop) [inventory]. Yapper diffs into `VecModel` with
inserts, removes and sets, never resetting, so `ListView` keeps its scroll position
(`timeline.rs`) [inventory].

**API (sketch):** `reconcile(model: &VecModel<T>, new: &[T], key: impl Fn(&T) -> K)` emitting
the minimal insert/remove/set operations, with a fast path for a common prefix and suffix.
Optionally a small `Coalescer` for time-based flushes; slinty-pi's 33 ms `TEXT_FLUSH` lives in
pi-core and is toolkit-agnostic.

**Steps:**
1. Write `reconcile` fresh from yapper's `timeline.rs` approach. The prefix/suffix idea also
   appears in Flectar's `RetainedModel`; reimplement it, don't read or copy that code (D4).
2. Property tests: for random old/new lists, applying the emitted operations to `old` yields
   `new`.
3. Adopt in yapper's timeline (replacing its local diff) and in slinty-pi's history reloads.

Acceptance: property tests; on the real backend (not headless, bummer: "headless backend
differs on element lifetime"), a reload keeps the scroll position in both apps.
Effort: 1–2 days (estimate).

## 11. Phase 6 — `slint-file-drop`

**Current state:** slinty-pi and yapper both use `on_winit_window_event` [read]. winit 0.30's
file-drop events carry no position. Yapper fills that in on macOS by reading the pointer
through AppKit, and rejects drops elsewhere [read]. slinty-pi doesn't need the position, since
it attaches to the composer.

**Plan:**
1. Small crate: `install_file_drop(window, on_hover: Fn(Option<LogicalPosition>), on_file:
   Fn(Option<LogicalPosition>, PathBuf, first: bool))` with yapper's `DragSession` batching and
   the macOS position workaround (feature `unstable-winit-030`).
2. **Upstream first, in parallel:** winit 0.31 (`0.31.0-beta.3`, 2026-09-04) moves to
   data-transfer drag events that carry positions [read]. Slint has an in-progress winit 0.31
   port (PRs #13489, #13710 merge master into `feature/winit-0.31`) [read: PR titles]; whether
   it maps external drops onto `DropArea` is unknown. Propose that it does, with positions. If
   accepted, this crate shrinks to nothing and both apps use `DropArea`.

Acceptance: a manual Finder drag in each app (hover highlight, drop, multi-file batch). Demo
modes must handle the attach commands first (slinty-pi's `demo_backend` ignores them).
Effort: half a day for the crate; the upstream proposal takes Till's time (estimate).

## 12. Phase 7 — `slint-widgets`

**Scope:** leaf components with explicit properties, not app row models. That sidesteps the
"slinty-pi's `Row` is one flat struct keyed by a `kind` string" blocker: a code block takes
`lines: [CodeLine]`, not a `Row`.

**Candidates:**
- `CommandPalette`: slinty-pi `palette.slint` plus `pi-core/src/palette.rs` ranking with
  nucleo-matcher 0.3 [inventory]; yapper's `SearchPalette`/`PalettePanel` [inventory].
- `CodeBlock` with language label and `CopyButton`: slinty-pi `CodeRow` [inventory].
- Design tokens global: semantic colours derived from std-widgets `Palette` so light and dark
  work. Yapper is dark-only today [inventory]. Flectar's "a few base colours generate the rest,
  WCAG-checked" is the idea to reimplement (D4).
- **SVG icon set**, e.g. Lucide (ISC) with its licence file. This replaces Unicode-symbol icons
  (bummer: "✕ ✎ ◷ drew as empty boxes"), including slinty-pi's remaining colour-emoji `↪`/`⚙︎`.

**Distribution (to verify in a spike):** the crate ships `ui/*.slint` and exposes its path; each
consumer's `build.rs` maps a library name to it with `slint_build::CompilerConfiguration::
with_library_paths`, and imports `@<lib>/palette.slint`.

**Steps:**
1. Spike the distribution mechanism with one component.
2. Design the token global and how apps override it.
3. Move components one at a time, each verified by before/after MCP screenshots in both apps.

Acceptance: per component, both apps import it from the shared crate and the screenshots match,
or the differences are intended and listed.
Effort: spike half a day; then about half a day per component (estimate).

## 13. Phase 8 — `local-llm`

**Sources:**
- slinty-pi `pi-local`:
  - `rapid_mlx.rs`: detection, catalog and cached models via `--json` with a text fallback, the
    serve lifecycle [read].
  - `router.rs`: llama.cpp router client.
  - `hf.rs`: Hugging Face GGUF search.
  - `ollama.rs`: Ollama detection.
  - `system_fit.rs`: RAM-fit estimate.
- yapper `yapper-assistant/src/provider.rs`: an OpenAI-compatible `/chat/completions` client with
  SSE streaming, falling back to non-streaming on 400/422/501, loopback-only [inventory].
- **Stays in slinty-pi:** `auth_json.rs`, `models_json.rs` (pi's config files) and `panel.rs`.

**What each app gains:** yapper can detect and start rapid-mlx and list models instead of
assuming a running server. slinty-pi gains only shared maintenance, since pi itself makes the
model calls. That is why this phase is last.

Acceptance: pi-local's live tests (rapid-mlx installed) pass against the moved code; yapper's
assistant tests pass with the shared provider.
Effort: 1–2 days (estimate).

## 14. Side tracks (outside the shared repo)

| Item | Where | Notes |
|---|---|---|
| Switch yapper's macOS-only keychain code (`yapper-config/src/secrets.rs`, `security-framework`) to `keyring` | yapper | Cross-platform; Flectar uses keyring for the same job [inventory]. Watch for re-prompting Keychain approvals after the switch (yapper bummer, 2026-09-27). |
| `license = "MIT"` metadata in yapper's crates | yapper | Workspace field plus `license.workspace = true`, except the AGPL crates. |
| slinty-pi `demo_backend`: handle `AttachPath`/`AttachImageData`/`SetDragHover` | slinty-pi | So demo mode can exercise phases 3 and 6. |
| Upstream: external file drops with position in Slint's winit 0.31 port | Slint | Phase 6, step 2. |
| Upstream: wrapped `StyledText` link hit-testing | Slint | Yapper bummer 2026-09-28; affects slinty-pi's prose links too. |
| Upstream: box-layout height-for-width (slint-ui/slint#12776) | Slint | Already filed; track. |

## 15. Order and dependencies

```
0 bootstrap ─┬─ 2 md-segments + emoji ──(yapper refactor timing)
             ├─ 3 clipboard
             ├─ 4 notify evaluation
             ├─ 5 model-sync
             ├─ 6 file-drop ──(Slint winit 0.31 outcome)
             ├─ 7 widgets ──(after 2 for CodeBlock's CodeLine type)
             └─ 8 local-llm
```

Recommended sequence: 0 → 2 → 3 → 4 → 5 → 7 → 8. Phase 6 waits on the upstream answer.
The total is roughly 8–12 working days spread over several sessions (estimate).

## 16. Three highest-risk decisions

1. **Slint lockstep across repos.** Every Slint-dependent shared crate pins the Slint version
   all its consumers must use. slinty-pi, yapper and Flectar are all on 1.18.1 today [read]. A
   Slint upgrade becomes a coordinated three-repo change. Mitigation: keep the Slint crates
   few and thin, keep pure logic in the pure crates (D5), and have CI build against the newest
   Slint release so breakage shows up early. (Versions checked in all three lockfiles.)
2. **Licensing hygiene is only partly mechanical.** cargo-deny enforces dependency licences and
   sources (D2), but not where code came from. Two manual risks: extracting yapper code
   publishes code from a currently private repo (only the generic parts, per D3), and
   "reimplement Flectar ideas" (D4) needs real clean-room discipline. Write from the
   description in this plan, with Flectar's source closed.
3. **Extracting during yapper's refactor.** `blocks.rs` and the timeline diff move inside
   yapper soon (`plan/refactoring.md`). Extracting in the middle of that creates double moves
   and merge pain. The `Segment` and `Spec` models may also never fit one shape. Mitigation:
   agree the timing in yapper's plan (phase 2, step 2) and keep model convergence a separate,
   optional decision.
