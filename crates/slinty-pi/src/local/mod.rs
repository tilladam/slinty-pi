//! Local-model backend foundation for M3 ("local models, delightfully"): the
//! llama.cpp router HTTP client, rapid-mlx CLI integration, and system
//! RAM-fit estimation. See `docs/plans/M3-local-models.md`.
//!
//! `rapid_mlx`/`system_fit` back the models panel's rapid-mlx section;
//! `router` backs the llama.cpp router section (item 4b) — list/load/unload
//! with progress polled from `GET /models`, verified via `backend.rs`'s
//! demo-mode fakes since there's no real llama-server on the dev machine.
//! `download_model` backs `hf`'s "Download model…" flow; `delete_model`/the
//! SSE stream aren't used yet — see their own `#[allow(dead_code)]` markers.

pub mod auth_json;
pub mod hf;
pub mod models_json;
pub mod ollama;
pub mod rapid_mlx;
pub mod router;
pub mod system_fit;
