//! Local-model backend foundation for M3 ("local models, delightfully"): the
//! llama.cpp router HTTP client, rapid-mlx CLI integration, and system
//! RAM-fit estimation. See `docs/plans/M3-local-models.md`.
//!
//! `rapid_mlx`/`system_fit` back the models panel's rapid-mlx section.
//! `router` isn't wired into the UI yet — that's the llama.cpp router
//! section (plan item 4b), which needs demo-mode fakes first since there's
//! no real llama-server on the dev machine to verify it against.

pub mod rapid_mlx;
#[allow(dead_code)] // wired in for the router section of the models panel (item 4b), not yet
pub mod router;
pub mod system_fit;
