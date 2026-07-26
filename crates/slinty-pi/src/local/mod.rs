//! Local-model backend foundation for M3 ("local models, delightfully"): the
//! llama.cpp router HTTP client, rapid-mlx CLI integration, and system
//! RAM-fit estimation. See `docs/plans/M3-local-models.md`.
//!
//! Not yet wired into `backend.rs`/the UI — this module lands the
//! network/process/parsing layer first so the models panel, onboarding
//! flow, and composer badges (later M3 work) have something solid to sit
//! on top of.

pub mod rapid_mlx;
pub mod router;
pub mod system_fit;
