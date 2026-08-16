# Milestone implementation plans

Implementation plans for the remaining milestones of [PRODUCT_PLAN.md](../../PRODUCT_PLAN.md).
Each plan is grounded in the current codebase (`pi-rpc` client, `backend.rs` Ui/Transcript/RowSpec
architecture, `app.slint` component set) and in verified facts about pi 0.81.x and Slint 1.17.

| Plan | Theme | Status |
|---|---|---|
| [M2 — Sessions & projects](M2-sessions-and-projects.md) | Never lose work; switch contexts instantly | planned |
| [M3 — Local models](M3-local-models.md) | The best local-LLM front door pi has | planned |
| [M4 — Agent trust](M4-agent-trust.md) | Extension dialogs, permissions, diffs, compaction | planned |
| [M5 — Ship it](M5-ship.md) | Settings, polish, packaging, 1.0 | planned |

These four are the Slint app's (`slint/slinty-pi`) plans. The native SwiftUI macOS app
(`macos/swifty-pi`) has its own build log instead of a milestone plan:
[swiftui-native-macos-app.md](swiftui-native-macos-app.md).

## M1 stragglers

Deferred from M1, to be folded into later milestones where they fit naturally:

- **File/image attach + drag & drop** (composer) → M2, alongside `@file` groundwork; uses Slint
  1.17 `DropArea` and pi's `images` field on `prompt`.
- **Per-message copy for prose** (code cards already have copy) → M2, as a hover affordance on
  message groups; copy source is the raw markdown buffer kept by the backend.
- **Editable queue chips** (currently display-only) → M4, needs a queue-edit RPC strategy
  (pi has no queue-remove command; likely abort + re-prompt composition).
- **App icon v1** → M5, with the packaging work.
- **Auto-scroll opt-out** (pin-to-bottom only when already at bottom) → M2, needs viewport state
  exposed from the ListView to the backend.

## Conventions

- Every milestone lands runnable and demoable; `cargo fmt` + `clippy --workspace` clean and
  `cargo test --workspace` green at every commit.
- The demo backend must keep exercising new UI paths (sessions, dialogs, diffs) so the rendering
  layer stays verifiable without a live model.
- pi remains the source of truth for all session state; slinty-pi's own persisted state
  (recents, window geometry, permission memory) lives in `~/Library/Application Support/slinty-pi/`
  (via the `directories` crate), never in pi's directories.
